//! Numeric literal typing, range checks, and literal decoding.

use super::*;

impl<'g> TypeChecker<'g> {
    /// A numeric literal gets a class variable; its range is checked at body completion.
    /// A leading sign is recorded with the magnitude for signed-minimum checks.
    pub(super) fn number_type(&mut self, text: &str, span: Span, negated: bool) -> Type {
        let ty = if let Some(literal) = self.body.literals.iter().find(|literal| literal.span == span) {
            literal.ty.clone()
        } else {
            let kind = if literal_is_float(text) { InferKind::FloatLiteral } else { InferKind::IntLiteral };
            let ty = self.infer.fresh(kind);
            self.body.literals.push(Literal { ty: ty.clone(), span, text: text.to_string(), negated });
            ty
        };
        ty
    }

    /// Report a diagnostic if the integer literal `text`, optionally `negated`, does
    /// not fit the range of integer type `name`. Values are staged through 128-bit
    /// arithmetic, wide enough to hold any 64-bit-or-narrower bound exactly.
    pub(super) fn check_integer_range(&mut self, text: &str, name: &str, negated: bool, span: Span) {
        let Some(mag) = parse_literal_magnitude(text) else {
            self.err(span, format!("integer literal `{text}` is too large to represent"));
            return;
        };
        let bits = integer_bits(name);
        if name.starts_with('u') {
            if negated && mag != 0 {
                self.err(span, format!("cannot negate a literal of unsigned type {name}"));
                return;
            }
            let max = (1u128 << bits) - 1;
            if mag > max {
                self.err(span, format!("integer literal {mag} is out of range for {name} (0..={max})"));
            }
        } else if negated {
            let min_magnitude = 1u128 << (bits - 1);
            if mag > min_magnitude {
                self.err(span, format!("integer literal -{mag} is out of range for {name} (min -{min_magnitude})"));
            }
        } else {
            let max = (1u128 << (bits - 1)) - 1;
            if mag > max {
                self.err(span, format!("integer literal {mag} is out of range for {name} (max {max})"));
            }
        }
    }
}

/// Whether an integer type is signed (admits negative literal patterns).
pub(super) fn is_signed_int(name: &str) -> bool {
    SIGNED_INTS.contains(&name)
}

/// Whether a numeric literal is floating-point. Hexadecimal and binary literals are
/// always integers; a decimal literal is floating-point when it carries a fractional
/// point or a decimal exponent.
pub(super) fn literal_is_float(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("0x") || lower.starts_with("0b") {
        return false;
    }
    lower.contains('.') || lower.contains('e')
}

/// The bare integer magnitude of a numeric literal, ignoring digit separators and
/// respecting a hexadecimal or binary prefix. `None` when the value overflows 128
/// bits — beyond any Cooper integer type, so already out of range.
fn parse_literal_magnitude(text: &str) -> Option<u128> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let lower = cleaned.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        u128::from_str_radix(hex, 16).ok()
    } else if let Some(bin) = lower.strip_prefix("0b") {
        u128::from_str_radix(bin, 2).ok()
    } else {
        cleaned.parse::<u128>().ok()
    }
}

/// A numeric literal's decoded value: a non-negative integer magnitude (any sign is
/// a surrounding unary operator, not part of the literal) or a floating-point value.
/// The literal's resolved type, carried on the IR node, fixes its width and signedness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LiteralValue {
    Int(u128),
    Float(f64),
}

/// Decode a string literal's source spelling (quotes included) into its contents,
/// resolving the escapes `\n`, `\t`, `\r`, `\0`, `\\`, `\"`, and `\'`. An unknown
/// escape is returned as the error, which the checker reports, so a clean check
/// guarantees `Ok`. Shared with lowering so the spelling is interpreted in one place.
pub fn decode_string_literal(text: &str) -> Result<String, String> {
    let inner = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    decode_escapes(inner)
}

/// Decode the escapes in a piece of string literal text, between its quotes and holes.
/// On failure, the unknown escape as written.
pub fn decode_escapes(inner: &str) -> Result<String, String> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let escaped = chars.next().unwrap_or('\\');
        out.push(match escaped {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '\\' | '"' | '\'' | '{' => escaped,
            other => return Err(format!("\\{other}")),
        });
    }
    Ok(out)
}

/// Decode a numeric literal's source spelling into its value, classifying it by the
/// same lexical rule the checker applies (a fractional point or decimal exponent
/// makes it floating-point). `None` only for text the checker would already have
/// rejected — an integer magnitude beyond 128 bits or an unparsable float — so a
/// clean check guarantees `Some`. Shared with lowering so literal spelling is
/// interpreted in exactly one place.
pub fn decode_number_literal(text: &str) -> Option<LiteralValue> {
    if literal_is_float(text) {
        let cleaned: String = text.chars().filter(|c| *c != '_').collect();
        cleaned.parse::<f64>().ok().map(LiteralValue::Float)
    } else {
        parse_literal_magnitude(text).map(LiteralValue::Int)
    }
}

/// The literal text of a bare numeric literal seen through groupings, or `None` for
/// any other expression — the shape a leading sign folds into for range checking.
pub(super) fn bare_number_text(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Number(text) => Some(text),
        ExprKind::Group(inner) => bare_number_text(inner),
        _ => None,
    }
}
