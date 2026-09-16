//! Import loading, the `.sigil` workspace graph, and `emitter ... from ...` composition with
//! parameter overrides (Plan 0002 WP4.2).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::compiler::diagnose::{diagnostic, unresolved_position};
use crate::compiler::model::{BulletView, EmitterView, FieldValue, FileView};
use crate::diagnostics::Diagnostic;
use crate::parser;

/// Resolves the source text of an imported path.
///
/// Implemented by the caller of [`crate::compiler::compile`] so this crate never touches a
/// filesystem itself (contract §1: `grimoire_sigilc` is an offline tool crate, not a runtime one,
/// but it still keeps I/O at its edges rather than baked into the compiler proper, the same way
/// `sigilc check`'s file reads live in `src/bin/sigilc.rs`, not in `crate::parser::parse`).
pub trait SourceLoader {
    /// Reads the source of `path` (as named by an `import "<path>" as <alias>` item, or the entry
    /// path passed to [`crate::compiler::compile`]).
    ///
    /// # Errors
    /// Returns [`LoadError`] if `path` cannot be read.
    fn load(&self, path: &str) -> Result<String, LoadError>;
}

/// Why [`SourceLoader::load`] failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LoadError {
    /// No source exists at this path.
    #[error("not found")]
    NotFound,
    /// The path exists but could not be read (e.g. an I/O error, a permissions problem).
    #[error("{0}")]
    Other(String),
}

/// Every `.sigil` file reachable from the entry file, parsed into [`FileView`]s.
pub(crate) struct Workspace {
    pub files: BTreeMap<String, FileView>,
}

/// `(importing_file, import_position, import_alias_token)` of the `import` that queued a file for
/// loading, or `None` for the entry file itself.
type ImportEdge = Option<(String, crate::span::Position, String)>;

/// Loads and parses `entry_path`/`entry_source` and every file it transitively imports.
///
/// Returns the built [`Workspace`] together with every parser diagnostic collected along the way
/// (contract §2 rule 1: still a usable, best-effort workspace even when some file has parse
/// errors, mirroring `crate::parser::parse`'s own fault tolerance) — unless the entry file itself
/// cannot even be assigned a path-keyed slot, which cannot actually happen here, so the `Err`
/// arm only ever carries diagnostics for a hard, unrecoverable loader failure on the entry path
/// itself.
pub(crate) fn load_workspace(
    entry_path: &str,
    entry_source: &str,
    loader: &dyn SourceLoader,
) -> Result<(Workspace, Vec<Diagnostic>), Vec<Diagnostic>> {
    let mut files: BTreeMap<String, FileView> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut queue: VecDeque<(String, ImportEdge)> = VecDeque::new();
    queue.push_back((entry_path.to_string(), None));
    let mut in_progress: BTreeSet<String> = BTreeSet::new();

    while let Some((path, imported_from)) = queue.pop_front() {
        if files.contains_key(&path) {
            continue;
        }
        let source = if path == entry_path {
            entry_source.to_string()
        } else {
            match loader.load(&path) {
                Ok(source) => source,
                Err(_) => {
                    if let Some((importer, position, token)) = imported_from {
                        diagnostics.push(diagnostic(
                            "SIG0012",
                            &importer,
                            position,
                            "imports",
                            &token,
                            format!("Cannot load imported file `{path}`."),
                            "Check the import path; it is relative to the same root the compiler was invoked with.",
                        ));
                    }
                    continue;
                }
            }
        };

        if !in_progress.insert(path.clone()) {
            // Already mid-load somewhere on this queue: a duplicate `import` of the same path is
            // fine (`load_workspace` de-duplicates by path above); this branch only guards
            // against a pathological loader, never against a real cycle (cycles are detected
            // below, from each file's own recorded imports once it is parsed).
        }

        let output = parser::parse(path.clone(), &source);
        diagnostics.extend(output.diagnostics);
        let view = FileView::build(&source, &output.tree);

        for (alias, (target, position)) in &view.imports {
            queue.push_back((
                target.clone(),
                Some((path.clone(), *position, alias.clone())),
            ));
        }
        files.insert(path, view);
    }

    // Import-cycle detection: a file that (transitively) imports itself. Walk each file's import
    // graph with its own visiting stack; report once per file where the cycle is first closed.
    for start in files.keys().cloned().collect::<Vec<_>>() {
        let mut stack = vec![start.clone()];
        let mut seen = BTreeSet::new();
        seen.insert(start.clone());
        if let Some(cycle_at) = find_cycle(&files, &start, &mut stack, &mut seen) {
            diagnostics.push(diagnostic(
                "SIG0013",
                &start,
                unresolved_position(),
                "imports",
                &cycle_at,
                format!("Import cycle detected: `{start}` transitively imports itself through `{cycle_at}`."),
                "Break the cycle by removing one of the imports involved.",
            ));
        }
    }

    Ok((Workspace { files }, diagnostics))
}

fn find_cycle(
    files: &BTreeMap<String, FileView>,
    current: &str,
    stack: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) -> Option<String> {
    let view = files.get(current)?;
    for (target, _) in view.imports.values() {
        if target == &stack[0] {
            return Some(current.to_string());
        }
        if seen.insert(target.clone()) {
            stack.push(target.clone());
            if let Some(found) = find_cycle(files, target, stack, seen) {
                return Some(found);
            }
            stack.pop();
        }
    }
    None
}

/// A fully composed emitter, plus, for every foreign bullet its `bullet` field (transitively)
/// pulls in, the file it came from.
pub(crate) struct ResolvedUnit {
    /// The entry file's own canonical path (contract §11.1's canonical content path), used for
    /// diagnostics that are not about any one field (a whole missing construct, a cascade cycle).
    pub entry_path: String,
    /// Composed emitters of the entry file, in name order (deterministic; contract §2 rule 10:
    /// "keine Iterationsreihenfolge ungeordneter Container in den Bytes").
    pub emitters: Vec<EmitterView>,
    /// Every bullet the compiled unit needs: the entry file's own bullets, plus any bullet
    /// (transitively) referenced by a composed emitter's `bullet` field from another file.
    /// `(origin_file, view)`, sorted by `(origin_file, name)`.
    pub bullets: Vec<(String, BulletView)>,
}

/// Resolves every top-level emitter of `entry_path` (composing `from`, if present) and the
/// closure of bullets those emitters and their transform chains need.
///
/// # Errors
/// Returns a single [`Diagnostic`] if `entry_path` was never loaded (only possible if the loader
/// itself failed on the entry path, already reported by [`load_workspace`]; kept as a `Result`
/// here purely so a caller cannot accidentally resolve a workspace missing its own entry).
pub(crate) fn resolve_entry(
    workspace: &Workspace,
    entry_path: &str,
) -> Result<ResolvedUnit, Box<Diagnostic>> {
    let Some(entry) = workspace.files.get(entry_path) else {
        return Err(Box::new(diagnostic(
            "SIG0012",
            entry_path,
            unresolved_position(),
            "",
            "",
            "The entry file itself could not be loaded.",
            "",
        )));
    };

    let mut emitters = Vec::new();
    for view in entry.emitters.values() {
        let mut stack = Vec::new();
        let composed = compose_emitter(workspace, entry_path, view, &mut stack);
        emitters.push(composed);
    }

    let mut bullets: BTreeMap<(String, String), BulletView> = BTreeMap::new();
    for (name, view) in &entry.bullets {
        bullets.insert(
            (entry_path.to_string(), name.clone()),
            tag_bullet_origin(view, entry_path),
        );
    }

    let mut worklist: VecDeque<(String, String)> = VecDeque::new();
    for emitter in &emitters {
        if let Some(bullet_field) = emitter.fields.get("bullet")
            && let FieldValue::Ident(name) = &bullet_field.value
        {
            let key = (bullet_field.origin_file.clone(), name.clone());
            if !bullets.contains_key(&key) {
                worklist.push_back(key);
            }
        }
    }
    while let Some((file, name)) = worklist.pop_front() {
        if bullets.contains_key(&(file.clone(), name.clone())) {
            continue;
        }
        let Some(source_file) = workspace.files.get(&file) else {
            continue; // unknown reference; `validate` reports this.
        };
        let Some(bullet) = source_file.bullets.get(&name) else {
            continue; // unknown reference; `validate` reports this.
        };
        let bullet = tag_bullet_origin(bullet, &file);
        for transform in &bullet.transforms {
            for field_name in ["to", "bullet"] {
                if let Some(located) = transform.fields.get(field_name)
                    && let FieldValue::Ident(target) = &located.value
                {
                    let key = (file.clone(), target.clone());
                    if !bullets.contains_key(&key) {
                        worklist.push_back(key);
                    }
                }
            }
        }
        bullets.insert((file, name), bullet);
    }

    Ok(ResolvedUnit {
        entry_path: entry_path.to_string(),
        emitters,
        bullets: bullets
            .into_iter()
            .map(|((file, _), v)| (file, v))
            .collect(),
    })
}

/// Composes `view` (declared in `file`) against its `from` base, if any, recursively.
///
/// `stack` guards against a composition cycle (`emitter a from b`, `emitter b from a`, possibly
/// across files): each `(file, emitter)` pair is pushed before recursing and popped after: if a
/// pair is seen twice, composition stops and returns `view` unmodified rather than looping
/// forever (`validate` reports the cycle as a diagnostic separately, since this function has no
/// way to surface one without a fallible signature every caller would otherwise have to plumb
/// through).
fn compose_emitter(
    workspace: &Workspace,
    file: &str,
    view: &EmitterView,
    stack: &mut Vec<(String, String)>,
) -> EmitterView {
    let mut view = tag_emitter_origin(view, file);
    let Some((alias, base_name, _position)) = &view.from else {
        return view;
    };
    let key = (file.to_string(), view.name.clone());
    if stack.contains(&key) {
        return view; // cycle; `validate` reports it.
    }

    let Some(base_file) = workspace.files.get(file).and_then(|f| f.imports.get(alias)) else {
        return view; // unknown alias; `validate` reports it.
    };
    let base_file = base_file.0.clone();
    let Some(base_view) = workspace
        .files
        .get(&base_file)
        .and_then(|f| f.emitters.get(base_name))
    else {
        return view; // unknown base emitter; `validate` reports it.
    };

    stack.push(key);
    let base_composed = compose_emitter(workspace, &base_file, base_view, stack);
    stack.pop();

    let mut composed = base_composed;
    composed.name = view.name.clone();
    composed.position = view.position;
    composed.from = view.from.take();

    for (path, located) in std::mem::take(&mut view.fields) {
        if let Some(subfield) = path.strip_prefix("block.") {
            if let Some(block) = &mut composed.block {
                block.fields.insert(subfield.to_string(), located);
            }
            // A `block.<field>` override with no base block is an error; `validate` reports it.
        } else {
            composed.fields.insert(path, located);
        }
    }
    if view.block.is_some() {
        composed.block = view.block.take();
    }
    composed.modifiers.append(&mut view.modifiers);

    composed
}

/// Fills in every field's `origin_file` with `file` (see [`Located::origin_file`]'s docs): a
/// [`FileView`] is built without knowing its own path at field-reading time, so this is applied
/// once, right when an emitter is first pulled into composition.
fn tag_emitter_origin(view: &EmitterView, file: &str) -> EmitterView {
    let mut view = view.clone();
    for located in view.fields.values_mut() {
        located.origin_file = file.to_string();
    }
    if let Some(block) = &mut view.block {
        for located in block.fields.values_mut() {
            located.origin_file = file.to_string();
        }
    }
    for modifier in &mut view.modifiers {
        for located in modifier.fields.values_mut() {
            located.origin_file = file.to_string();
        }
    }
    view
}

/// Fills in `origin_file` for a bullet's own fields and its transforms' fields, the same way
/// [`tag_emitter_origin`] does for an emitter.
pub(crate) fn tag_bullet_origin(view: &BulletView, file: &str) -> BulletView {
    let mut view = view.clone();
    for located in view.fields.values_mut() {
        located.origin_file = file.to_string();
    }
    for transform in &mut view.transforms {
        for located in transform.fields.values_mut() {
            located.origin_file = file.to_string();
        }
        if let Some(block) = &mut transform.block {
            for located in block.fields.values_mut() {
                located.origin_file = file.to_string();
            }
        }
    }
    view
}
