//! Type descriptors for the garbage collector: which words of a value are pointers.
//!
//! Every heap allocation is made through `cooper_alloc(size, type)`. `type` describes one value
//! of the type allocated; an allocation of several values (an array's storage) is that
//! description tiled. A null `type` means the allocation holds no pointers, so the collector
//! never scans it: string bytes, and arrays of numbers.
//!
//! The descriptor is a constant `{ size, layout size, flags, words, bitmap }` (the C
//! `CooperType`): the value's size in bytes as LLVM computes it, the size this module's own layout
//! computation arrived at (the runtime checks that they agree, which validates that layout), flags,
//! the number of 8-byte words it covers, and one bit per word, set when the word holds a pointer. A sum type's payload words hold a pointer or not depending on the variant:
//! its bitmap marks every word some variant uses as a pointer, and its flags say so (bit 0), for
//! the collector to treat as a superset until it scans by tag.

use cooper_frontend::types::{integer_bits, is_float_name, is_integer_name, Type};

use crate::{mangle, Module};

/// The size, alignment, and pointer words of a value, laid out as LLVM does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    pub size: usize,
    pub align: usize,
    /// The byte offsets of the pointers in a value, ascending.
    pub pointers: Vec<usize>,
    /// Whether some pointer words are only pointers for some variants of a sum type.
    pub tagged: bool,
}

impl Layout {
    fn scalar(size: usize) -> Layout {
        Layout { size, align: size, pointers: Vec::new(), tagged: false }
    }

    fn pointers(count: usize) -> Layout {
        Layout { size: count * 8, align: 8, pointers: (0..count).map(|i| i * 8).collect(), tagged: false }
    }

    /// The layout of `fields` laid out one after another, as an LLVM struct is.
    fn sequence(fields: &[Layout]) -> Layout {
        let mut out = Layout { size: 0, align: 1, pointers: Vec::new(), tagged: false };
        for field in fields {
            out.size = out.size.next_multiple_of(field.align.max(1));
            out.pointers.extend(field.pointers.iter().map(|p| p + out.size));
            out.tagged |= field.tagged;
            out.size += field.size;
            out.align = out.align.max(field.align);
        }
        out.size = out.size.next_multiple_of(out.align);
        out
    }
}

impl Module<'_> {
    /// The layout of a value of type `ty`.
    pub(crate) fn gc_layout(&self, ty: &Type) -> Layout {
        match ty {
            Type::Unit => Layout { size: 0, align: 1, pointers: Vec::new(), tagged: false },
            Type::Primitive(name) if name == "string" => Layout::pointers(1).then_scalar(8),
            Type::Primitive(name) if name == "bool" => Layout::scalar(1),
            Type::Primitive(name) if name == "rune" => Layout::scalar(4),
            Type::Primitive(name) if is_integer_name(name) => Layout::scalar(integer_bits(name) as usize / 8),
            Type::Primitive(name) if is_float_name(name) => Layout::scalar(if name == "f32" { 4 } else { 8 }),
            Type::Primitive(_) => Layout::scalar(8),
            Type::Pointer(_) | Type::Nil => Layout::pointers(1),
            // A function value is its code and its environment.
            Type::Func { .. } => Layout::pointers(2),
            // The elements, their count, and the capacity.
            Type::Array(_) => Layout::pointers(1).then_scalar(8).then_scalar(8),
            Type::StaticArray(elem, len) => {
                let elem = self.gc_layout(elem);
                let mut out = Layout { size: 0, align: elem.align, pointers: Vec::new(), tagged: elem.tagged };
                for index in 0..*len as usize {
                    out.pointers.extend(elem.pointers.iter().map(|p| p + index * elem.size));
                }
                out.size = elem.size.saturating_mul(*len as usize);
                out
            }
            Type::Tuple(elems) => Layout::sequence(&elems.iter().map(|e| self.gc_layout(e)).collect::<Vec<_>>()),
            Type::Struct { .. } => {
                let members = self.defs.struct_members(ty).unwrap_or_default();
                Layout::sequence(&members.iter().map(|(_, m)| self.gc_layout(m)).collect::<Vec<_>>())
            }
            Type::Oneof { .. } => {
                // `{ i32 tag, [words x i64] payload }`: the payload starts at byte 8, and holds
                // each variant's slots laid out as a struct.
                let mut words = 0;
                let mut pointers: Vec<usize> = Vec::new();
                let mut varies = false;
                for variant in self.defs.variant_order(ty) {
                    let slots = self.defs.payload(ty, variant).unwrap_or_default();
                    let payload = Layout::sequence(&slots.iter().map(|s| self.gc_layout(s)).collect::<Vec<_>>());
                    words = words.max(self.size_bound(&Type::Tuple(slots)).div_ceil(8));
                    varies |= payload.tagged || !payload.pointers.is_empty();
                    pointers.extend(payload.pointers.iter().map(|p| p + 8));
                }
                pointers.sort_unstable();
                pointers.dedup();
                Layout { size: 8 + words * 8, align: 8, pointers, tagged: varies }
            }
            Type::Unknown | Type::Infer(_) | Type::TypeParam(_) | Type::Module(_) => Layout::default(),
        }
    }

    /// The descriptor operand (`ptr @name`'s name) for allocations of values of type `ty`, or
    /// `None` when they hold no pointers.
    pub(crate) fn type_descriptor(&mut self, ty: &Type) -> Option<String> {
        let layout = self.gc_layout(ty);
        if layout.pointers.is_empty() {
            return None;
        }
        let name = format!("@cooper.type.{}", mangle::type_name(ty));
        let llvm = self.llvm_type(ty).ok().flatten()?;
        self.define_descriptor(&name, &llvm, &layout);
        Some(name)
    }

    /// The descriptor of `count` pointers in a row (a closure's environment).
    pub(crate) fn pointers_descriptor(&mut self, count: usize) -> Option<String> {
        if count == 0 {
            return None;
        }
        let name = format!("@cooper.type.pointers{count}");
        self.define_descriptor(&name, &format!("[{count} x ptr]"), &Layout::pointers(count));
        Some(name)
    }

    fn define_descriptor(&mut self, name: &str, llvm: &str, layout: &Layout) {
        if self.descriptors.contains_key(name) {
            return;
        }
        let words = layout.size.div_ceil(8);
        let mut bitmap = vec![0u64; words.div_ceil(64).max(1)];
        for pointer in &layout.pointers {
            debug_assert!(pointer % 8 == 0, "a pointer is word-aligned");
            bitmap[pointer / 8 / 64] |= 1 << (pointer / 8 % 64);
        }
        let bits = bitmap.iter().map(|b| format!("i64 {}", *b as i64)).collect::<Vec<_>>().join(", ");
        self.descriptors.insert(
            name.to_string(),
            format!(
                "{name} = private constant {{ i64, i64, i64, i64, [{} x i64] }} {{ i64 ptrtoint (ptr getelementptr ({llvm}, ptr null, i32 1) to i64), i64 {}, i64 {}, i64 {words}, [{} x i64] [{bits}] }}\n",
                bitmap.len(),
                layout.size,
                i64::from(layout.tagged),
                bitmap.len()
            ),
        );
    }
}

impl Layout {
    /// This layout followed by a scalar of `size` bytes.
    fn then_scalar(self, size: usize) -> Layout {
        Layout::sequence(&[self, Layout::scalar(size)])
    }
}
