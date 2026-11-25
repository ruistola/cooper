//! What a module's generic declarations need of their type parameters, and whether each
//! instantiation meets it.
//!
//! A body's operations put requirements on its declaration's type parameters directly; a
//! call that passes a type parameter on to another generic declaration passes that one's
//! requirements on too. Bodies are checked before every requirement of the module is known,
//! so the requirements settle and the instantiations are checked once the whole module is.

use std::collections::HashMap;

use super::solve::{holds, params_needing, unmet};
use super::{Instantiation, Typed};
use crate::diag::{line_col, Diagnostic};
use crate::resolve::{Callable, Globals, Requirement};

/// Settle the requirements of the generic declarations of the module whose files (name and
/// source) checked into `typed`, recording them in its `interface`, and report every
/// instantiation that does not meet its callee's. `dependencies` are the interfaces of the
/// modules analyzed before this one.
pub(crate) fn settle<'a>(
    files: &[(&str, &str)],
    typed: &[&Typed],
    interface: &mut Globals,
    dependencies: impl Iterator<Item = &'a Globals>,
) -> Vec<Diagnostic> {
    let mut imported: HashMap<&Callable, &[Requirement]> = HashMap::new();
    for globals in dependencies {
        for sig in globals.funcs.values().chain(globals.methods.values().flat_map(|methods| methods.values())) {
            imported.insert(&sig.item, &sig.requirements);
        }
    }

    // The compiler provides some declarations with their requirements.
    let mut own: HashMap<Callable, Vec<Requirement>> = (interface.funcs.values())
        .filter(|sig| !sig.requirements.is_empty())
        .map(|sig| (sig.item.clone(), sig.requirements.clone()))
        .collect();
    for ((name, source), typed) in files.iter().zip(typed) {
        for (item, requirement, span) in &typed.requirements {
            let (line, col) = line_col(source, span.start);
            let origin = format!("{} at {name}:{line}:{col}", requirement.origin);
            add(own.entry(item.clone()).or_default(), Requirement { origin, ..requirement.clone() });
        }
    }
    let params: HashMap<Callable, Vec<String>> = (interface.funcs.values())
        .chain(interface.methods.values().flat_map(|methods| methods.values()))
        .filter(|sig| !sig.type_params.is_empty())
        .map(|sig| (sig.item.clone(), sig.type_params.clone()))
        .collect();
    let instantiations: Vec<(&str, &Instantiation)> = (files.iter().zip(typed))
        .flat_map(|((name, _), typed)| typed.instantiations.iter().map(move |instance| (*name, instance)))
        .collect();

    // A requirement passed on can only name one of the finitely many parameter and
    // predicate pairs of its caller, so this terminates.
    loop {
        let mut added = false;
        for (_, instance) in &instantiations {
            let Some(caller) = &instance.caller else { continue };
            let required = own.get(&instance.callee).cloned().unwrap_or_else(|| {
                imported.get(&instance.callee).map_or_else(Vec::new, |requirements| requirements.to_vec())
            });
            for requirement in required {
                let arg = argument(instance, &requirement);
                let Some(Ok(names)) = params_needing(&requirement.kind, arg, &interface.defs) else { continue };
                for param in names.into_iter().filter(|name| params[caller].contains(name)) {
                    let origin = format!("calls {}, which {}", instance.label, requirement.origin);
                    added |= add(own.entry(caller.clone()).or_default(), Requirement { param, kind: requirement.kind.clone(), origin });
                }
            }
        }
        if !added {
            break;
        }
    }

    let mut diags = Vec::new();
    for (file, instance) in &instantiations {
        let required = own.get(&instance.callee).map(Vec::as_slice).or_else(|| imported.get(&instance.callee).copied());
        for requirement in required.unwrap_or_default() {
            let arg = argument(instance, requirement);
            let met = match params_needing(&requirement.kind, arg, &interface.defs) {
                None => holds(&requirement.kind, arg, &interface.defs),
                Some(Ok(names)) => instance.caller.as_ref().is_some_and(|caller| names.iter().all(|name| params[caller].contains(name))),
                Some(Err(())) => false,
            };
            // The first unmet requirement explains the error; more would repeat it.
            if !met {
                let note = format!("{} {}", instance.label, requirement.origin);
                diags.push(Diagnostic::error(instance.span, unmet(&requirement.kind, arg, &interface.defs)).with_note(note).in_file(*file));
                break;
            }
        }
    }

    for sig in interface.funcs.values_mut().chain(interface.methods.values_mut().flat_map(|methods| methods.values_mut())) {
        if let Some(requirements) = own.remove(&sig.item) {
            sig.requirements = requirements;
        }
    }
    diags
}

fn argument<'i>(instance: &'i Instantiation, requirement: &Requirement) -> &'i crate::types::Type {
    let (_, arg) = (instance.type_args.iter())
        .find(|(name, _)| *name == requirement.param)
        .expect("a requirement names one of its declaration's type parameters");
    arg
}

/// Add `requirement` unless the same predicate is already required of its parameter;
/// whether it was added.
fn add(requirements: &mut Vec<Requirement>, requirement: Requirement) -> bool {
    let known = requirements.iter().any(|r| r.param == requirement.param && r.kind == requirement.kind);
    if !known {
        requirements.push(requirement);
    }
    !known
}
