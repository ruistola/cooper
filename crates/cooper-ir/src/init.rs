//! Initialization order of module-level variables.
//!
//! Modules initialize depth first: a module's variables run after those of every module
//! it depends on, which is the order the checked project lists them in. Within a module
//! the order follows Go's: repeatedly take the earliest declared variable that does not
//! depend on a variable still uninitialized. A variable depends on every module-level
//! variable its initializer mentions, directly or through the functions it references,
//! so declaration order never matters where a dependency exists. A variable that
//! depends on itself has no valid order and is reported.

use std::collections::{HashMap, HashSet};

use cooper_frontend::resolve::Callable;

use crate::mono::dependencies;
use crate::{Function, IrExprKind, IrStmt, IrStmtKind, LowerError, ProgramError, Variable};

/// The initializer functions of `variables` in the order they run. `variables` lists
/// the program's variables module by module, in dependency order; `functions` holds
/// every function the program declares, templates included.
pub(crate) fn order(variables: &[Variable], functions: &[Function]) -> Result<Vec<Callable>, ProgramError> {
    let bodies: HashMap<&Callable, &Function> = functions.iter().map(|f| (&f.item, f)).collect();
    let mut order = Vec::new();
    let mut start = 0;
    while start < variables.len() {
        let module = &variables[start].global.module;
        let end = variables[start..]
            .iter()
            .position(|v| &v.global.module != module)
            .map_or(variables.len(), |len| start + len);
        order.extend(module_order(&variables[start..end], &bodies)?);
        start = end;
    }
    Ok(order)
}

/// The initializers of one module's `variables` in their run order.
fn module_order(
    variables: &[Variable],
    bodies: &HashMap<&Callable, &Function>,
) -> Result<Vec<Callable>, ProgramError> {
    let mut pending: Vec<(&Variable, Callable, HashSet<String>)> = Vec::new();
    for variable in variables.iter().filter(|v| v.initialized) {
        let item = Callable::Initializer {
            module: variable.global.module.clone(),
            name: variable.global.name.clone(),
        };
        let uses = reached_variables(&item, bodies);
        let deps: HashSet<String> = variables
            .iter()
            .filter(|v| v.initialized && uses.contains(&(v.global.module.clone(), v.global.name.clone())))
            .map(|v| v.global.name.clone())
            .collect();
        pending.push((variable, item, deps));
    }
    let mut order = Vec::new();
    let mut done: HashSet<&str> = HashSet::new();
    while !pending.is_empty() {
        let Some(ready) = pending
            .iter()
            .position(|(_, _, deps)| deps.iter().all(|dep| done.contains(dep.as_str())))
        else {
            let (variable, ..) = &pending[0];
            return Err(ProgramError::Lower {
                file: variable.file.clone(),
                error: LowerError::InitializationCycle {
                    span: variable.span,
                    name: variable.global.name.clone(),
                },
            });
        };
        let (variable, item, _) = pending.remove(ready);
        done.insert(&variable.global.name);
        order.push(item);
    }
    Ok(order)
}

/// Every module-level variable, as (module, name), that running the initializer `item`
/// may read or write, following references to functions transitively. The store of the
/// initializer's own result is not a use.
fn reached_variables(item: &Callable, bodies: &HashMap<&Callable, &Function>) -> HashSet<(String, String)> {
    let mut variables = HashSet::new();
    let mut seen: HashSet<Callable> = HashSet::from([item.clone()]);
    let mut work = Vec::new();
    if let Some(function) = bodies.get(item) {
        let (callables, used) = dependencies(&initializer_value(function));
        work.extend(callables);
        variables.extend(used);
    }
    while let Some(callable) = work.pop() {
        if !seen.insert(callable.clone()) {
            continue;
        }
        if let Some(function) = bodies.get(&callable) {
            let (callables, used) = dependencies(&function.body);
            work.extend(callables);
            variables.extend(used);
        }
    }
    variables
}

/// The statements evaluating an initializer's value: the right side of the store that
/// is the initializer function's whole body.
fn initializer_value(function: &Function) -> Vec<IrStmt> {
    match function.body.as_slice() {
        [IrStmt { kind: IrStmtKind::Expr(store), span }] => match &store.kind {
            IrExprKind::Assign { value, .. } => vec![IrStmt {
                kind: IrStmtKind::Expr((**value).clone()),
                span: *span,
            }],
            _ => unreachable!("an initializer stores its value"),
        },
        _ => unreachable!("an initializer is a single store"),
    }
}
