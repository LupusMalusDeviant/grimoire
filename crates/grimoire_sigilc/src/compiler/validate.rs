//! Static validation (Plan 0002 WP4.2): unknown references, parameter ranges (including
//! NaN/Infinity-generating boundary values), the reserved `beats` unit, cascade depth and
//! recursion, and the PRD-0003 readability rules. Continues the diagnostic-code sequence from
//! `SIG0012` (`docs/formats/sigil.md`'s table has the authoritative list; this file's comments
//! explain the *why* behind each check, not a duplicate of that table).

use std::collections::BTreeMap;

use crate::catalog;
use crate::compiler::diagnose::diagnostic;
use crate::compiler::model::{BlockView, FieldValue, Located, ModifierView, TransformView};
use crate::compiler::resolve::{ResolvedUnit, Workspace};
use crate::diagnostics::Diagnostic;
use crate::span::Position;

/// Runs every static check over an already-resolved (imports loaded, `from` composed) unit.
pub(crate) fn validate(
    workspace: &Workspace,
    resolved: &ResolvedUnit,
    behavior_ids: &BTreeMap<String, u32>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    for (file, bullet) in &resolved.bullets {
        validate_bullet(workspace, file, bullet, behavior_ids, &mut out);
    }
    for emitter in &resolved.emitters {
        validate_emitter(workspace, &resolved.entry_path, emitter, &mut out);
    }
    validate_cascade(resolved, &mut out);
    validate_silhouette_readability(resolved, &mut out);

    out
}

// ---- Small typed-field helpers --------------------------------------------------------------

/// A field's expected shape, checked generically by [`expect`] so every construct's field list
/// below reads as plain data instead of repeating the same `match` by hand.
enum Expect {
    Ident,
    /// A dotted reference with exactly this many segments (e.g. `2` for `enemy.crimson`).
    Ref(usize),
    Float,
    Int,
    /// A quantity with exactly this unit (contract: Sigil quantities carry one unit per field).
    Quantity(&'static str),
    /// `repeat`'s special shape: a positive integer, or the literal identifier `forever`.
    RepeatCount,
}

/// Reads and type-checks one field of `fields`, reporting [`Diagnostic`]s for a wrong shape or a
/// missing-but-required field. Returns `None` on any problem (already reported), so callers can
/// use `if let Some(value) = ...` to only proceed with a value they can trust the shape of.
fn expect<'a>(
    fields: &'a BTreeMap<String, Located<FieldValue>>,
    name: &str,
    shape: Expect,
    required: bool,
    entry_file: &str,
    out: &mut Vec<Diagnostic>,
) -> Option<&'a Located<FieldValue>> {
    let Some(located) = fields.get(name) else {
        if required {
            out.push(diagnostic(
                "SIG0023",
                entry_file,
                Position { line: 1, column: 1 },
                name,
                "",
                format!("Missing required field `{name}`."),
                format!("Add `{name} = ...`."),
            ));
        }
        return None;
    };
    let report_file = if located.origin_file.is_empty() {
        entry_file
    } else {
        located.origin_file.as_str()
    };
    let ok = match (&shape, &located.value) {
        (Expect::Ident, FieldValue::Ident(_)) => true,
        (Expect::Ref(n), FieldValue::Ref(segments)) => segments.len() == *n,
        (Expect::Float, FieldValue::Float(_) | FieldValue::Int(_)) => true,
        (Expect::Int, FieldValue::Int(_)) => true,
        (Expect::Quantity(unit), FieldValue::Quantity(_, actual_unit)) => {
            if actual_unit == "beats" {
                out.push(diagnostic(
                    "SIG0015",
                    report_file,
                    located.position,
                    located.node_path.clone(),
                    "beats",
                    format!(
                        "Field `{name}` uses the reserved unit `beats`; Sigil has no wall-clock \
                         units, only ticks (`t`) and world units (`u`)."
                    ),
                    format!("Use `{unit}` instead."),
                ));
                return None;
            }
            actual_unit == unit
        }
        (Expect::RepeatCount, FieldValue::Int(_)) => true,
        (Expect::RepeatCount, FieldValue::Ident(text)) => text == "forever",
        (_, FieldValue::Invalid) => return None, // already reported by the parser.
        _ => false,
    };
    if !ok {
        out.push(diagnostic(
            "SIG0024",
            report_file,
            located.position,
            located.node_path.clone(),
            debug_token(&located.value),
            format!("Field `{name}` has the wrong shape for this construct."),
            expected_hint(name, &shape),
        ));
        return None;
    }
    Some(located)
}

fn expected_hint(name: &str, shape: &Expect) -> String {
    match shape {
        Expect::Ident => format!("`{name}` must be a plain identifier."),
        Expect::Ref(n) => format!("`{name}` must be a {n}-segment dotted reference."),
        Expect::Float => format!("`{name}` must be a number."),
        Expect::Int => format!("`{name}` must be a whole number."),
        Expect::Quantity(unit) => format!("`{name}` must be a `{unit}` quantity."),
        Expect::RepeatCount => format!("`{name}` must be a positive whole number, or `forever`."),
    }
}

fn debug_token(value: &FieldValue) -> String {
    match value {
        FieldValue::Ident(s) | FieldValue::Str(s) => s.clone(),
        FieldValue::Ref(segments) => segments.join("."),
        FieldValue::Int(v) => v.to_string(),
        FieldValue::Float(v) => v.to_string(),
        FieldValue::Quantity(v, unit) => format!("{v}{unit}"),
        FieldValue::List(_) => "[...]".to_string(),
        FieldValue::Record(_) => "(...)".to_string(),
        FieldValue::Trigger(keyword, _) => keyword.clone(),
        FieldValue::Invalid => String::new(),
    }
}

fn float_of(value: &FieldValue) -> f64 {
    match value {
        FieldValue::Float(v) => *v,
        FieldValue::Int(v) => *v as f64,
        FieldValue::Quantity(v, _) => *v,
        _ => 0.0,
    }
}

/// Reports a plain out-of-range value (contract: "Parameterbereiche").
fn range_error(
    file: &str,
    located: &Located<FieldValue>,
    field_name: &str,
    requirement: &str,
    out: &mut Vec<Diagnostic>,
) {
    out.push(diagnostic(
        "SIG0016",
        file,
        located.position,
        located.node_path.clone(),
        debug_token(&located.value),
        format!("Field `{field_name}` {requirement}."),
        format!("Change `{field_name}` so it {requirement}."),
    ));
}

/// Reports a value that would make a documented downstream formula divide by zero (contract:
/// "NaN-erzeugende Grenzwerte" -- e.g. an angular-spacing formula computing `360° / count`).
fn nan_risk_error(
    file: &str,
    located: &Located<FieldValue>,
    field_name: &str,
    formula: &str,
    out: &mut Vec<Diagnostic>,
) {
    out.push(diagnostic(
        "SIG0017",
        file,
        located.position,
        located.node_path.clone(),
        debug_token(&located.value),
        format!(
            "Field `{field_name}` is zero, which makes {formula} divide by zero (a non-finite \
             result is never allowed in compiled Sigil data)."
        ),
        format!("Use a positive value for `{field_name}`."),
    ));
}

// ---- Bullets -----------------------------------------------------------------------------

/// `SIG0027`: a silhouette or enemy palette name that is not in the visual catalogue
/// (`crate::catalog`, `docs/formats/sigil.md` §10.9). Without a catalogue row the name has no
/// index that means the same thing in every unit, so it cannot be compiled.
fn unknown_visual(
    file: &str,
    located: &Located<FieldValue>,
    what: &str,
    name: &str,
    table: &[&str],
    out: &mut Vec<Diagnostic>,
) {
    out.push(diagnostic(
        "SIG0027",
        file,
        located.position,
        located.node_path.clone(),
        debug_token(&located.value),
        format!(
            "{what} `{name}` is not in the visual catalogue (docs/formats/sigil.md section 10.9)."
        ),
        format!("Use one of: {}.", table.join(", ")),
    ));
}

fn validate_bullet(
    workspace: &Workspace,
    file: &str,
    bullet: &crate::compiler::model::BulletView,
    behavior_ids: &BTreeMap<String, u32>,
    out: &mut Vec<Diagnostic>,
) {
    let fields = &bullet.fields;
    if let Some(silhouette) = expect(fields, "silhouette", Expect::Ident, true, file, out)
        && let FieldValue::Ident(name) = &silhouette.value
        && catalog::silhouette_index(name).is_none()
    {
        unknown_visual(
            file,
            silhouette,
            "Silhouette",
            name,
            catalog::SILHOUETTES,
            out,
        );
    }

    if let Some(palette) = expect(fields, "palette", Expect::Ref(2), true, file, out)
        && let FieldValue::Ref(segments) = &palette.value
    {
        if segments[0] == "enemy" {
            if catalog::enemy_palette_index(&segments[1]).is_none() {
                unknown_visual(
                    file,
                    palette,
                    "Enemy palette",
                    &segments[1],
                    catalog::ENEMY_PALETTES,
                    out,
                );
            }
        } else {
            out.push(diagnostic(
                "SIG0021",
                file,
                palette.position,
                palette.node_path.clone(),
                segments.join("."),
                format!(
                    "Sigil bullets may only reference the enemy palette space, found \
                         `{}` (PRD-0003 rule 4: enemy and player projectiles are told apart by \
                         separate, unmistakable palette spaces).",
                    segments[0]
                ),
                "Use an `enemy.<color>` palette reference.",
            ));
        }
    }

    if let Some(glow) = expect(fields, "glow", Expect::Float, true, file, out) {
        let value = float_of(&glow.value);
        if !(0.0..=1.0).contains(&value) {
            range_error(file, glow, "glow", "must be between 0.0 and 1.0", out);
        }
    }

    if let Some(radius) = expect(fields, "radius", Expect::Quantity("u"), true, file, out) {
        let value = float_of(&radius.value);
        if value <= 0.0 {
            range_error(file, radius, "radius", "must be greater than 0", out);
        }
    }

    if let Some(damage) = expect(fields, "damage", Expect::Int, true, file, out)
        && let FieldValue::Int(value) = damage.value
        && value < 0
    {
        range_error(file, damage, "damage", "must not be negative", out);
    }

    if let Some(flags) = fields.get("flags") {
        if let FieldValue::List(entries) = &flags.value {
            for entry in entries {
                if let FieldValue::Ident(name) = entry
                    && !matches!(
                        name.as_str(),
                        "smashable" | "reflectable" | "env_active" | "grazeable"
                    )
                {
                    out.push(diagnostic(
                        "SIG0022",
                        file,
                        flags.position,
                        flags.node_path.clone(),
                        name.clone(),
                        format!("Unknown bullet flag `{name}`."),
                        "Use one of: smashable, reflectable, env_active, grazeable.",
                    ));
                }
            }
        } else {
            range_error(file, flags, "flags", "must be a list of flag names", out);
        }
    } else {
        out.push(diagnostic(
            "SIG0023",
            file,
            bullet.position,
            format!("bullets.{}.flags", bullet.name),
            "",
            "Missing required field `flags` (an empty list `[]` is fine if there are none).",
            "Add `flags = [...]`.",
        ));
    }

    if let Some(despawn_vfx) = fields.get("despawn_vfx")
        && !matches!(despawn_vfx.value, FieldValue::Ident(_))
    {
        range_error(
            file,
            despawn_vfx,
            "despawn_vfx",
            "must be a plain identifier",
            out,
        );
    }

    if let Some(behaviour) = fields.get("behaviour")
        && let FieldValue::Ident(name) = &behaviour.value
        && !behavior_ids.contains_key(name)
    {
        out.push(diagnostic(
            "SIG0014",
            file,
            behaviour.position,
            behaviour.node_path.clone(),
            name.clone(),
            format!("Unknown behaviour `{name}`: no `BehaviorId` was supplied for this name."),
            "Register this name's BehaviorId with the compiler invocation, or fix the typo.",
        ));
    }

    for transform in &bullet.transforms {
        validate_transform(workspace, file, transform, out);
    }
}

/// PRD-0003 rule 3 ("Bullet-Typen unterscheiden sich durch Silhouette + Farbe, nie nur Farbe"):
/// no two bullet types compiled into the same unit may share a silhouette, even if their palette
/// differs -- a same-silhouette, different-color pair is exactly the failure mode the rule
/// forbids (a colorblind or fast-moving player cannot tell them apart by silhouette alone).
/// Checked across the whole compiled unit, not per file, since that is the unit players actually
/// see on screen together.
/// `(file, bullet_name, the `silhouette` field itself)`, grouped by silhouette name.
type SilhouetteUses<'a> = BTreeMap<String, Vec<(&'a str, &'a str, &'a Located<FieldValue>)>>;

fn validate_silhouette_readability(resolved: &ResolvedUnit, out: &mut Vec<Diagnostic>) {
    let mut by_silhouette: SilhouetteUses<'_> = BTreeMap::new();
    for (file, bullet) in &resolved.bullets {
        if let Some(located) = bullet.fields.get("silhouette")
            && let FieldValue::Ident(name) = &located.value
        {
            by_silhouette.entry(name.clone()).or_default().push((
                file.as_str(),
                bullet.name.as_str(),
                located,
            ));
        }
    }
    for (silhouette, entries) in by_silhouette {
        if entries.len() < 2 {
            continue;
        }
        let names: Vec<String> = entries
            .iter()
            .map(|(file, name, _)| format!("{file}:{name}"))
            .collect();
        for (file, name, located) in &entries {
            out.push(diagnostic(
                "SIG0020",
                file,
                located.position,
                located.node_path.clone(),
                silhouette.clone(),
                format!(
                    "Bullet `{name}` shares silhouette `{silhouette}` with {} other bullet \
                     type(s) in this unit ({}); PRD-0003 rule 3 requires bullet types to differ \
                     in silhouette, not only in color.",
                    entries.len() - 1,
                    names.join(", "),
                ),
                "Give each bullet type its own silhouette, or merge them if they are really the same type.",
            ));
        }
    }
}

fn validate_trigger(file: &str, located: &Located<FieldValue>, out: &mut Vec<Diagnostic>) {
    let FieldValue::Trigger(keyword, argument) = &located.value else {
        range_error(
            file,
            located,
            "when",
            "must be a time/distance/event trigger",
            out,
        );
        return;
    };
    match keyword.as_str() {
        "time" => match argument.as_ref() {
            FieldValue::Quantity(v, unit) if unit == "t" => {
                if *v < 0.0 {
                    range_error(file, located, "when", "must not be negative", out);
                }
            }
            FieldValue::Quantity(_, unit) if unit == "beats" => out.push(diagnostic(
                "SIG0015",
                file,
                located.position,
                located.node_path.clone(),
                "beats",
                "`time` triggers use the reserved unit `beats`; Sigil has no wall-clock units.",
                "Use a `t` quantity instead.",
            )),
            _ => range_error(
                file,
                located,
                "when",
                "a `time` trigger needs a `t` quantity",
                out,
            ),
        },
        "distance" => {
            if !matches!(argument.as_ref(), FieldValue::Quantity(_, unit) if unit == "u") {
                range_error(
                    file,
                    located,
                    "when",
                    "a `distance` trigger needs a `u` quantity",
                    out,
                );
            }
        }
        "event" => {
            if !matches!(argument.as_ref(), FieldValue::Ident(_) | FieldValue::Ref(_)) {
                range_error(
                    file,
                    located,
                    "when",
                    "an `event` trigger needs a name",
                    out,
                );
            }
        }
        _ => range_error(
            file,
            located,
            "when",
            "must be `time`, `distance` or `event`",
            out,
        ),
    }
}

fn validate_transform(
    workspace: &Workspace,
    file: &str,
    transform: &TransformView,
    out: &mut Vec<Diagnostic>,
) {
    let fields = &transform.fields;
    if let Some(when) = fields.get("when") {
        validate_trigger(file, when, out);
    } else {
        out.push(diagnostic(
            "SIG0023",
            file,
            transform.kind_position,
            "when",
            "",
            "Missing required field `when`.",
            "Add `when = time <t> | distance <u> | event <name>`.",
        ));
    }

    let target_file = workspace.files.get(file);
    match transform.kind.as_str() {
        "reverse" => {}
        "become_emitter" => {
            if let Some(emitter) = expect(fields, "emitter", Expect::Ident, true, file, out)
                && let FieldValue::Ident(name) = &emitter.value
            {
                let known = target_file.is_some_and(|f| f.emitters.contains_key(name));
                if !known {
                    unknown_reference(file, emitter, "emitter", name, out);
                }
            }
        }
        "change_type" => {
            if let Some(to) = expect(fields, "to", Expect::Ident, true, file, out)
                && let FieldValue::Ident(name) = &to.value
            {
                let known = target_file.is_some_and(|f| f.bullets.contains_key(name));
                if !known {
                    unknown_reference(file, to, "bullet", name, out);
                }
            }
        }
        "burst" => {
            if let Some(bullet) = expect(fields, "bullet", Expect::Ident, true, file, out)
                && let FieldValue::Ident(name) = &bullet.value
            {
                let known = target_file.is_some_and(|f| f.bullets.contains_key(name));
                if !known {
                    unknown_reference(file, bullet, "bullet", name, out);
                }
            }
            expect(fields, "speed", Expect::Quantity("u/t"), true, file, out);
            if let Some(block) = &transform.block {
                validate_block(file, block, out);
            } else {
                out.push(diagnostic(
                    "SIG0023",
                    file,
                    transform.kind_position,
                    "block",
                    "",
                    "A `burst` transform needs a nested `block { ... }`.",
                    "Add `block <kind> { ... }`.",
                ));
            }
        }
        other => out.push(diagnostic(
            "SIG0022",
            file,
            transform.kind_position,
            "",
            other.to_string(),
            format!("Unknown transform kind `{other}`."),
            "Use one of: reverse, become_emitter, change_type, burst.",
        )),
    }
}

fn unknown_reference(
    file: &str,
    located: &Located<FieldValue>,
    what: &str,
    name: &str,
    out: &mut Vec<Diagnostic>,
) {
    out.push(diagnostic(
        "SIG0014",
        file,
        located.position,
        located.node_path.clone(),
        name.to_string(),
        format!("Unknown {what} `{name}`."),
        format!("Define a {what} named `{name}`, or fix the typo."),
    ));
}

// ---- Emitters ------------------------------------------------------------------------------

fn validate_emitter(
    workspace: &Workspace,
    entry_path: &str,
    emitter: &crate::compiler::model::EmitterView,
    out: &mut Vec<Diagnostic>,
) {
    let fields = &emitter.fields;
    if let Some(bullet) = expect(fields, "bullet", Expect::Ident, true, entry_path, out)
        && let FieldValue::Ident(name) = &bullet.value
    {
        let known = workspace
            .files
            .get(&bullet.origin_file)
            .is_some_and(|f| f.bullets.contains_key(name));
        if !known {
            unknown_reference(&bullet.origin_file, bullet, "bullet", name, out);
        }
    }

    if let Some(role) = fields.get("role")
        && !matches!(&role.value, FieldValue::Ident(name) if name == "sub")
    {
        out.push(diagnostic(
            "SIG0022",
            &role.origin_file,
            role.position,
            role.node_path.clone(),
            debug_token(&role.value),
            "Unknown emitter role.",
            "The only supported role is `sub`; omit the field for a primary emitter.",
        ));
    }

    if let Some(delay) = fields.get("delay") {
        expect_quantity_field(delay, "delay", "t", out);
        if let FieldValue::Quantity(v, unit) = &delay.value
            && unit == "t"
            && *v < 0.0
        {
            range_error(
                &delay.origin_file,
                delay,
                "delay",
                "must not be negative",
                out,
            );
        }
    }

    if let Some(repeat) = expect(fields, "repeat", Expect::RepeatCount, true, entry_path, out)
        && let FieldValue::Int(value) = repeat.value
        && value < 1
    {
        range_error(
            &repeat.origin_file,
            repeat,
            "repeat",
            "must be at least 1",
            out,
        );
    }

    if let Some(interval) = expect(
        fields,
        "interval",
        Expect::Quantity("t"),
        true,
        entry_path,
        out,
    ) {
        let value = float_of(&interval.value);
        if value <= 0.0 {
            nan_risk_error(
                &interval.origin_file,
                interval,
                "interval",
                "the emitter's tick-scheduling formula (`tick % interval`)",
                out,
            );
        }
    }

    if let Some(speed) = expect(
        fields,
        "speed",
        Expect::Quantity("u/t"),
        true,
        entry_path,
        out,
    ) && float_of(&speed.value) < 0.0
    {
        range_error(
            &speed.origin_file,
            speed,
            "speed",
            "must not be negative",
            out,
        );
    }

    if let Some(offset) = fields.get("offset") {
        if let FieldValue::Record(entries) = &offset.value {
            for (name, value) in entries {
                if name != "x" && name != "y" {
                    out.push(diagnostic(
                        "SIG0022",
                        &offset.origin_file,
                        offset.position,
                        offset.node_path.clone(),
                        name.clone(),
                        format!("Unknown `offset` field `{name}`."),
                        "`offset` only has `x` and `y`.",
                    ));
                } else if !matches!(value, FieldValue::Quantity(_, unit) if unit == "u") {
                    range_error(
                        &offset.origin_file,
                        offset,
                        "offset",
                        "must have `x`/`y` as `u` quantities",
                        out,
                    );
                }
            }
        } else {
            range_error(
                &offset.origin_file,
                offset,
                "offset",
                "must be a record `(x = .., y = ..)`",
                out,
            );
        }
    }

    match &emitter.block {
        Some(block) => validate_block(&emitter_block_file(emitter), block, out),
        None => out.push(diagnostic(
            "SIG0023",
            entry_path,
            emitter.position,
            format!("emitters.{}.block", emitter.name),
            "",
            "Missing required nested `block { ... }`.",
            "Add `block <kind> { ... }`.",
        )),
    }

    for modifier in &emitter.modifiers {
        validate_modifier(modifier, out);
    }
}

/// The file a composed emitter's block should report diagnostics against: the file the block's
/// own fields actually live in (they all share one `origin_file` once composed; empty only for a
/// pathological emitter with an empty block, which [`validate_block`] itself flags separately).
fn emitter_block_file(emitter: &crate::compiler::model::EmitterView) -> String {
    emitter
        .block
        .as_ref()
        .and_then(|b| b.fields.values().next())
        .map(|located| located.origin_file.clone())
        .unwrap_or_default()
}

fn expect_quantity_field(
    located: &Located<FieldValue>,
    name: &str,
    unit: &str,
    out: &mut Vec<Diagnostic>,
) {
    match &located.value {
        FieldValue::Quantity(_, actual) if actual == "beats" => out.push(diagnostic(
            "SIG0015",
            &located.origin_file,
            located.position,
            located.node_path.clone(),
            "beats",
            format!("Field `{name}` uses the reserved unit `beats`."),
            format!("Use `{unit}` instead."),
        )),
        FieldValue::Quantity(_, actual) if actual == unit => {}
        _ => range_error(
            &located.origin_file,
            located,
            name,
            &format!("must be a `{unit}` quantity"),
            out,
        ),
    }
}

// ---- Blocks and modifiers ------------------------------------------------------------------

fn validate_block(file: &str, block: &BlockView, out: &mut Vec<Diagnostic>) {
    let fields = &block.fields;
    match block.kind.as_str() {
        "ring" => {
            check_positive_count(file, fields, "count", out);
            check_angle(file, fields, "start", false, out);
        }
        "spiral" => {
            check_positive_count(file, fields, "arms", out);
            check_angle(file, fields, "step", true, out);
            check_angle(file, fields, "start", false, out);
        }
        "fan" => {
            check_positive_count(file, fields, "count", out);
            check_angle(file, fields, "spread", true, out);
            check_angle(file, fields, "center", false, out);
        }
        "aimed" => {
            check_positive_count(file, fields, "count", out);
            check_angle(file, fields, "spread", false, out);
        }
        "wave" => {
            check_positive_count(file, fields, "count", out);
            check_quantity(file, fields, "amplitude", "u", out);
            if let Some(wavelength) = fields.get("wavelength") {
                expect_quantity_field(wavelength, "wavelength", "u", out);
                if float_of(&wavelength.value) == 0.0 {
                    nan_risk_error(
                        file,
                        wavelength,
                        "wavelength",
                        "the wave's phase formula (`x / wavelength`)",
                        out,
                    );
                } else if float_of(&wavelength.value) < 0.0 {
                    range_error(
                        file,
                        wavelength,
                        "wavelength",
                        "must be greater than 0",
                        out,
                    );
                }
            } else {
                missing(file, "wavelength", block.kind_position, out);
            }
            check_angle(file, fields, "direction", false, out);
        }
        "line" => {
            check_positive_count(file, fields, "count", out);
            check_quantity(file, fields, "spacing", "u", out);
            check_angle(file, fields, "direction", false, out);
        }
        "scatter" => {
            check_positive_count(file, fields, "count", out);
            check_angle(file, fields, "cone", true, out);
            check_angle(file, fields, "direction", false, out);
            if let Some(jitter) = fields.get("speed_jitter") {
                if !matches!(jitter.value, FieldValue::Float(_) | FieldValue::Int(_)) {
                    range_error(file, jitter, "speed_jitter", "must be a number", out);
                } else if !(0.0..=1.0).contains(&float_of(&jitter.value)) {
                    range_error(
                        file,
                        jitter,
                        "speed_jitter",
                        "must be between 0.0 and 1.0",
                        out,
                    );
                }
            } else {
                missing(file, "speed_jitter", block.kind_position, out);
            }
            if let Some(seed) = fields.get("seed") {
                if !matches!(seed.value, FieldValue::Ident(_)) {
                    range_error(file, seed, "seed", "must be a plain identifier", out);
                }
            } else {
                missing(file, "seed", block.kind_position, out);
            }
        }
        other => out.push(diagnostic(
            "SIG0022",
            file,
            block.kind_position,
            "block",
            other.to_string(),
            format!("Unknown block kind `{other}`."),
            "Use one of: ring, spiral, fan, aimed, wave, line, scatter.",
        )),
    }
}

fn validate_modifier(modifier: &ModifierView, out: &mut Vec<Diagnostic>) {
    let fields = &modifier.fields;
    let file = fields
        .values()
        .next()
        .map(|l| l.origin_file.clone())
        .unwrap_or_default();
    match modifier.kind.as_str() {
        "accelerate" => {
            check_quantity(&file, fields, "rate", "u/t2", out);
            check_quantity(&file, fields, "max_speed", "u/t", out);
        }
        "sine_offset" => {
            check_quantity(&file, fields, "amplitude", "u", out);
            if let Some(period) = fields.get("period") {
                expect_quantity_field(period, "period", "t", out);
                if float_of(&period.value) == 0.0 {
                    nan_risk_error(
                        &file,
                        period,
                        "period",
                        "the sine phase formula (`age / period`)",
                        out,
                    );
                } else if float_of(&period.value) < 0.0 {
                    range_error(&file, period, "period", "must be greater than 0", out);
                }
            } else {
                missing(&file, "period", modifier.kind_position, out);
            }
            check_angle(&file, fields, "phase", false, out);
        }
        "rotate" => {
            check_quantity(&file, fields, "rate", "deg/t", out);
        }
        "mirror" => {
            check_angle(&file, fields, "axis", true, out);
            if let Some(folds) = fields.get("folds") {
                match folds.value {
                    FieldValue::Int(0) => {
                        nan_risk_error(
                            &file,
                            folds,
                            "folds",
                            "the mirror's per-fold angle formula (`360° / folds`)",
                            out,
                        );
                    }
                    FieldValue::Int(value) if value < 0 => {
                        range_error(&file, folds, "folds", "must be at least 1", out);
                    }
                    FieldValue::Int(_) => {}
                    _ => range_error(&file, folds, "folds", "must be a whole number", out),
                }
            } else {
                missing(&file, "folds", modifier.kind_position, out);
            }
        }
        "speed_curve" => {
            if let Some(keys) = fields.get("keys") {
                if let FieldValue::List(entries) = &keys.value {
                    if entries.is_empty() {
                        range_error(&file, keys, "keys", "must have at least one keyframe", out);
                    }
                    for entry in entries {
                        if let FieldValue::Record(record_fields) = entry {
                            let has_at = record_fields.iter().any(|(n, v)| {
                                n == "at" && matches!(v, FieldValue::Quantity(_, u) if u == "t")
                            });
                            let has_mul = record_fields.iter().any(|(n, v)| {
                                n == "mul" && matches!(v, FieldValue::Float(_) | FieldValue::Int(_))
                            });
                            if !has_at || !has_mul {
                                range_error(
                                    &file,
                                    keys,
                                    "keys",
                                    "each entry must be `(at = <t>, mul = <number>)`",
                                    out,
                                );
                            }
                        } else {
                            range_error(&file, keys, "keys", "each entry must be a record", out);
                        }
                    }
                } else {
                    range_error(&file, keys, "keys", "must be a list of keyframes", out);
                }
            } else {
                missing(&file, "keys", modifier.kind_position, out);
            }
            if let Some(interp) = fields.get("interp") {
                if !matches!(&interp.value, FieldValue::Ident(name) if name == "linear" || name == "smooth")
                {
                    out.push(diagnostic(
                        "SIG0022",
                        &file,
                        interp.position,
                        interp.node_path.clone(),
                        debug_token(&interp.value),
                        "Unknown `interp` mode.",
                        "Use `linear` or `smooth`.",
                    ));
                }
            } else {
                missing(&file, "interp", modifier.kind_position, out);
            }
        }
        "curve" => {
            check_quantity(&file, fields, "turn", "deg/t", out);
        }
        other => out.push(diagnostic(
            "SIG0022",
            &file,
            modifier.kind_position,
            "modifier",
            other.to_string(),
            format!("Unknown modifier kind `{other}`."),
            "Use one of: accelerate, sine_offset, rotate, mirror, speed_curve, curve.",
        )),
    }
}

fn missing(file: &str, name: &str, position: Position, out: &mut Vec<Diagnostic>) {
    out.push(diagnostic(
        "SIG0023",
        file,
        position,
        name,
        "",
        format!("Missing required field `{name}`."),
        format!("Add `{name} = ...`."),
    ));
}

fn check_quantity(
    file: &str,
    fields: &BTreeMap<String, Located<FieldValue>>,
    name: &str,
    unit: &str,
    out: &mut Vec<Diagnostic>,
) {
    match fields.get(name) {
        Some(located) => expect_quantity_field(located, name, unit, out),
        None => missing(file, name, Position { line: 1, column: 1 }, out),
    }
}

/// Checks a `deg`-quantity angle field. Angles that feed a divisor formula (`required_nonzero`)
/// additionally reject exactly zero as a [`nan_risk_error`] (contract: "NaN-erzeugende
/// Grenzwerte") -- e.g. `fan.spread == 0` would make the per-shot angular step `spread / (count -
/// 1)` divide by zero for `count > 1`.
fn check_angle(
    file: &str,
    fields: &BTreeMap<String, Located<FieldValue>>,
    name: &str,
    required_nonzero: bool,
    out: &mut Vec<Diagnostic>,
) {
    let Some(located) = fields.get(name) else {
        return; // every angle field validated this way is optional (defaults to 0).
    };
    expect_quantity_field(located, name, "deg", out);
    if required_nonzero && float_of(&located.value) == 0.0 {
        nan_risk_error(
            file,
            located,
            name,
            "its per-shot angular-step formula",
            out,
        );
    }
}

fn check_positive_count(
    file: &str,
    fields: &BTreeMap<String, Located<FieldValue>>,
    name: &str,
    out: &mut Vec<Diagnostic>,
) {
    let Some(located) = fields.get(name) else {
        missing(file, name, Position { line: 1, column: 1 }, out);
        return;
    };
    match located.value {
        FieldValue::Int(0) => {
            nan_risk_error(
                file,
                located,
                name,
                "the angular-step formula (`360° / count`)",
                out,
            );
        }
        FieldValue::Int(value) if value < 0 => {
            range_error(file, located, name, "must be at least 1", out);
        }
        FieldValue::Int(_) => {}
        _ => range_error(file, located, name, "must be a whole number", out),
    }
}

// ---- Cascade depth and recursion -------------------------------------------------------------

/// Maximum sub-spawn cascade depth (contract §11.1/§11.3, `SigilUnit::MAX_CASCADE_DEPTH`).
/// Duplicated here (not imported) the same way `grimoire_sigilc::UNIT_ID_DOMAIN` duplicates its
/// own constant: this crate has no dependency edge that would let it read a `grimoire_sigil`
/// associated `const` at this point in the module graph, and the value is part of the frozen v1
/// contract, not something this compiler could get out of sync with by accident.
const MAX_CASCADE_DEPTH: u32 = 3;

/// Walks every primary (non-`sub`) emitter's spawn chain (`become_emitter`/`burst` increase the
/// cascade level by one; `change_type`/`reverse` do not) and reports [`SIG0018`] the first time a
/// chain would need a level beyond [`MAX_CASCADE_DEPTH`], and [`SIG0019`] if a bullet is reachable
/// from itself (a `change_type` cycle, since `become_emitter`/`burst` edges strictly increase the
/// level and so can never themselves close a cycle within the depth this function still walks).
fn validate_cascade(resolved: &ResolvedUnit, out: &mut Vec<Diagnostic>) {
    let bullets: BTreeMap<(&str, &str), &crate::compiler::model::BulletView> = resolved
        .bullets
        .iter()
        .map(|(file, view)| ((file.as_str(), view.name.as_str()), view))
        .collect();
    let emitters: BTreeMap<&str, &crate::compiler::model::EmitterView> = resolved
        .emitters
        .iter()
        .map(|view| (view.name.as_str(), view))
        .collect();

    for emitter in &resolved.emitters {
        let is_sub = matches!(
            emitter.fields.get("role").map(|l| &l.value),
            Some(FieldValue::Ident(name)) if name == "sub"
        );
        if is_sub {
            continue;
        }
        let Some(bullet_field) = emitter.fields.get("bullet") else {
            continue;
        };
        let FieldValue::Ident(bullet_name) = &bullet_field.value else {
            continue;
        };
        let key = (bullet_field.origin_file.as_str(), bullet_name.as_str());
        let mut stack = Vec::new();
        walk_cascade(key, 0, &bullets, &emitters, &mut stack, out);
    }
}

fn walk_cascade(
    key: (&str, &str),
    level: u32,
    bullets: &BTreeMap<(&str, &str), &crate::compiler::model::BulletView>,
    emitters: &BTreeMap<&str, &crate::compiler::model::EmitterView>,
    stack: &mut Vec<(String, String)>,
    out: &mut Vec<Diagnostic>,
) {
    let owned_key = (key.0.to_string(), key.1.to_string());
    if stack.contains(&owned_key) {
        out.push(diagnostic(
            "SIG0019",
            key.0,
            Position { line: 1, column: 1 },
            format!("bullets.{}", key.1),
            key.1.to_string(),
            format!(
                "Cascade cycle: bullet `{}` is reachable from itself through a chain of \
                 `change_type` transforms.",
                key.1
            ),
            "Break the cycle; a bullet's `change_type` chain must eventually stop.",
        ));
        return;
    }
    let Some(bullet) = bullets.get(&key) else {
        return; // unknown reference; already reported elsewhere.
    };
    stack.push(owned_key);

    for transform in &bullet.transforms {
        match transform.kind.as_str() {
            "change_type" => {
                if let Some(FieldValue::Ident(to)) = transform.fields.get("to").map(|l| &l.value) {
                    walk_cascade((key.0, to.as_str()), level, bullets, emitters, stack, out);
                }
            }
            "become_emitter" => {
                if let Some(FieldValue::Ident(target_emitter)) =
                    transform.fields.get("emitter").map(|l| &l.value)
                    && let Some(next_level) = check_level(level, transform, key.0, out)
                    && let Some(emitter) = emitters.get(target_emitter.as_str())
                    && let Some(FieldValue::Ident(next_bullet)) =
                        emitter.fields.get("bullet").map(|l| &l.value)
                {
                    let origin = emitter
                        .fields
                        .get("bullet")
                        .map(|l| l.origin_file.as_str())
                        .unwrap_or(key.0);
                    walk_cascade(
                        (origin, next_bullet.as_str()),
                        next_level,
                        bullets,
                        emitters,
                        stack,
                        out,
                    );
                }
            }
            "burst" => {
                if let Some(FieldValue::Ident(next_bullet)) =
                    transform.fields.get("bullet").map(|l| &l.value)
                    && let Some(next_level) = check_level(level, transform, key.0, out)
                {
                    walk_cascade(
                        (key.0, next_bullet.as_str()),
                        next_level,
                        bullets,
                        emitters,
                        stack,
                        out,
                    );
                }
            }
            _ => {}
        }
    }

    stack.pop();
}

/// Returns `Some(level + 1)` if that is still within [`MAX_CASCADE_DEPTH`], otherwise reports
/// [`SIG0018`] and returns `None` so the caller stops descending along this edge.
fn check_level(
    level: u32,
    transform: &TransformView,
    file: &str,
    out: &mut Vec<Diagnostic>,
) -> Option<u32> {
    let next = level + 1;
    if next > MAX_CASCADE_DEPTH {
        out.push(diagnostic(
            "SIG0018",
            file,
            transform.kind_position,
            "",
            transform.kind.clone(),
            format!(
                "This `{}` would create a bullet at cascade depth {next}, beyond the maximum of \
                 {MAX_CASCADE_DEPTH}.",
                transform.kind
            ),
            "Shorten the cascade chain, or remove this transform.",
        ));
        return None;
    }
    Some(next)
}
