You are an independent second rater of compiler diagnostics in a syntax experiment. Do not open,
search or run anything: everything you need is in this prompt. Do not look for other ratings.

Two text syntaxes for the same bullet-pattern data model are compared: "RON" (serde RON with unit
newtypes like `Ticks(20)`, `Deg(12.0)`) and "sigil 1" (own line-oriented grammar, `name = value`,
units as suffix like `20t`, `12deg`, `0.09u/t`). Ten error cases were injected into correct pattern
files, each changing exactly one place. For each case you get the injected change and the complete
diagnostic output of the prototype checker for each syntax (the first `error[...]` line is the
diagnostic's Cause; the `= help:` line is its Fix hint; `= note:` and `= path:` are extra context;
a missing `= help:` line means there is no fix hint).

Rate the FIRST diagnostic of each file on two columns, each 0-3, using exactly this rubric:

- Cause: 0 = none or wrong; 1 = misleading or generic; 2 = names the problem without context;
  3 = names the problem with context (field, kind, counterpart, concrete replacement).
- Fix hint: 0 = none or wrong; 1 = misleading or generic; 2 = names the problem without context;
  3 = names the problem with context (field, kind, counterpart, concrete replacement).

Judge what a modder who made exactly this mistake would learn from the message. Position and node
path are scored automatically elsewhere; do not score them, but a wrong error class counts against
the Cause.

Reply with exactly one Markdown table and nothing else, 20 rows in the order given, columns:
| case | syntax | cause | fix hint | reason (one short sentence) |
with `syntax` written as `RON` or `sigil 1`.


## e01: Tippfehler im Schlüssel (base pattern 01-ring-burst)

### e01 RON

Injected change (unified diff against the correct file):

```diff
@@ -34 +34 @@
-                count: 24,
+                cuont: 24,
```

Checker output:

```text
error[schema/unknown-field]: Unexpected field named `cuont` in `Ring`, expected either `count` or `start` instead.
  --> corpus/errors/e01-key-typo.ron:34:17
   |
34 |                 cuont: 24,
   |                 ^
  = path: emitters.burst.block.cuont
  = help: Did you mean `count`? Allowed fields of `Ring`: count, start.
```

### e01 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -30 +30 @@
-    count = 24
+    cuont = 24
```

Checker output:

```text
error[schema/unknown-field]: Unknown field `cuont` in block `ring`.
  --> corpus/errors/e01-key-typo.sigil:30:5
   |
30 |     cuont = 24
   |     ^
  = path: emitters.burst.block.cuont
  = help: Did you mean `count`? Allowed fields of block `ring`: count, start.
```


## e02: Falscher Typ (Gleitkomma statt Ganzzahl) (base pattern 02-aimed-stream)

### e02 RON

Injected change (unified diff against the correct file):

```diff
@@ -43 +43 @@
-                count: 3,
+                count: 3.5,
```

Checker output:

```text
error[parse/syntax]: Expected comma.
  --> corpus/errors/e02-wrong-type.ron:43:25
   |
43 |                 count: 3.5,
   |                         ^
  = path: emitters.stream.block.count
```

### e02 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -39 +39 @@
-    count = 3
+    count = 3.5
```

Checker output:

```text
error[schema/type]: Field `count` expects an integer, found the float `3.5`.
  --> corpus/errors/e02-wrong-type.sigil:39:13
   |
39 |     count = 3.5
   |             ^
  = path: emitters.stream.block.count
  = help: Use a whole number, e.g. `count = 3`.
```


## e03: Fehlendes Pflichtfeld (base pattern 04-mirrored-spiral)

### e03 RON

Injected change (unified diff against the correct file):

```diff
@@ -31 +30,0 @@
-            speed: UnitsPerTick(0.09),
```

Checker output:

```text
error[schema/missing-field]: Emitter `bloom` is missing the required field `speed`.
  --> corpus/errors/e03-missing-field.ron:27:19
   |
27 |             name: "bloom",
   |                   ^
  = note: `ron` reports the end of the struct at 55:9
  = path: emitters.bloom.speed
  = help: Add `speed: UnitsPerTick(<value>),` to Emitter `bloom`.
```

### e03 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -27 +26,0 @@
-  speed = 0.09u/t
```

Checker output:

```text
error[schema/missing-field]: Emitter `bloom` is missing the required field `speed`.
  --> corpus/errors/e03-missing-field.sigil:23:9
   |
23 | emitter bloom {
   |         ^
  = path: emitters.bloom.speed
  = help: Add a line `speed = <value>u/t`.
```


## e04: Nicht geschlossene Klammer (base pattern 03-subemitter-cascade)

### e04 RON

Injected change (unified diff against the correct file):

```diff
@@ -32 +31,0 @@
-        ),
```

Checker output:

```text
error[schema/unknown-field]: Unexpected field named `Bullet` in `Bullet`, expected one of `name`, `silhouette`, `palette`, `glow`, `radius`, `damage`, `flags`, `despawn_vfx`, `behaviour`, or `transforms` instead.
  --> corpus/errors/e04-unbalanced-bracket.ron:33:9
   |
33 |         Bullet(
   |         ^
  = path: bullets.seed
  = help: Allowed fields of `Bullet`: name, silhouette, palette, glow, radius, damage, flags, despawn_vfx, behaviour, transforms.
```

### e04 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -28 +27,0 @@
-}
```

Checker output:

```text
error[parse/unclosed-brace]: `bullet` cannot start a member inside `bullet seed`; the `{` opened at line 17 is not closed.
  --> corpus/errors/e04-unbalanced-bracket.sigil:30:1
   |
30 | bullet shard {
   | ^
  = note: opening brace at 17:13
  = path: bullets.seed
  = help: Insert `}` on its own line before this line.
```


## e05: Wert außerhalb des Bereichs (base pattern 01-ring-burst)

### e05 RON

Injected change (unified diff against the correct file):

```diff
@@ -34 +34 @@
-                count: 24,
+                count: 0,
```

Checker output:

```text
error[validate/range]: `count` of block `Ring` must be in 1..=512, found 0; a block of 0 bullets fires nothing and has no defined angle step.
  --> corpus/errors/e05-out-of-range.ron:34:24
   |
34 |                 count: 0,
   |                        ^
  = path: emitters.burst.block.count
  = help: Use 1 to 512 bullets, e.g. `count: 24,`.
```

### e05 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -30 +30 @@
-    count = 24
+    count = 0
```

Checker output:

```text
error[validate/range]: `count` of block `ring` must be in 1..=512, found 0; a block of 0 bullets fires nothing and has no defined angle step.
  --> corpus/errors/e05-out-of-range.sigil:30:13
   |
30 |     count = 0
   |             ^
  = path: emitters.burst.block.count
  = help: Use 1 to 512 bullets, e.g. `count = 24`.
```


## e06: Unbekannte Behaviour-ID (base pattern 05-wave-line-composite)

### e06 RON

Injected change (unified diff against the correct file):

```diff
@@ -43 +43 @@
-            behaviour: "orbit_parent",
+            behaviour: "seek_target_weak2",
```

Checker output:

```text
error[validate/unknown-behaviour]: Unknown behaviour `seek_target_weak2`; registered behaviours: orbit_parent, seek_target_weak.
  --> corpus/errors/e06-unknown-behaviour.ron:43:24
   |
43 |             behaviour: "seek_target_weak2",
   |                        ^
  = path: bullets.satellite.behaviour
  = help: Did you mean `seek_target_weak`? New behaviours must be registered in Rust first (FR-10).
```

### e06 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -38 +38 @@
-  behaviour = orbit_parent
+  behaviour = seek_target_weak2
```

Checker output:

```text
error[validate/unknown-behaviour]: Unknown behaviour `seek_target_weak2`; registered behaviours: orbit_parent, seek_target_weak.
  --> corpus/errors/e06-unknown-behaviour.sigil:38:15
   |
38 |   behaviour = seek_target_weak2
   |               ^
  = path: bullets.satellite.behaviour
  = help: Did you mean `seek_target_weak`? New behaviours must be registered in Rust first (FR-10).
```


## e07: Falsche Einheit (Ticks statt Grad) (base pattern 02-aimed-stream)

### e07 RON

Injected change (unified diff against the correct file):

```diff
@@ -44 +44 @@
-                spread: Deg(12.0), // total spread of the volley around the aim line
+                spread: Ticks(12), // total spread of the volley around the aim line
```

Checker output:

```text
error[schema/unit]: Field `spread` expects `Deg(..)`, found `Ticks(..)`.
  --> corpus/errors/e07-wrong-unit.ron:44:25
   |
44 |                 spread: Ticks(12), // total spread of the volley around the aim line
   |                         ^
  = path: emitters.stream.block.spread
  = help: Write `spread: Deg(12.0),`.
```

### e07 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -40 +40 @@
-    spread = 12deg // total spread of the volley around the aim line
+    spread = 12t // total spread of the volley around the aim line
```

Checker output:

```text
error[schema/unit]: Field `spread` expects an angle in `deg`, found the tick quantity `12t`.
  --> corpus/errors/e07-wrong-unit.sigil:40:14
   |
40 |     spread = 12t // total spread of the volley around the aim line
   |              ^
  = path: emitters.stream.block.spread
  = help: Write `spread = 12deg`.
```


## e08: Doppelter Emitter-Name (base pattern 05-wave-line-composite)

### e08 RON

Injected change (unified diff against the correct file):

```diff
@@ -66 +66 @@
-            name: "rail",
+            name: "tide",
```

Checker output:

```text
error[validate/duplicate-name]: Duplicate emitter name `tide`; first defined at line 48.
  --> corpus/errors/e08-duplicate-emitter.ron:66:19
   |
66 |             name: "tide",
   |                   ^
  = note: first definition at 48:19
  = path: emitters[1].name
  = help: Rename one of the emitters; emitter names must be unique within a unit.
```

### e08 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -57 +57 @@
-emitter rail {
+emitter tide {
```

Checker output:

```text
error[validate/duplicate-name]: Duplicate emitter name `tide`; first defined at line 41.
  --> corpus/errors/e08-duplicate-emitter.sigil:57:9
   |
57 | emitter tide {
   |         ^
  = note: first definition at 41:9
  = path: emitters[1].name
  = help: Rename one of the emitters; emitter names must be unique within a unit.
```


## e09: Komma-Fehler (doppeltes Komma in Liste) (base pattern 04-mirrored-spiral)

### e09 RON

Injected change (unified diff against the correct file):

```diff
@@ -49 +49 @@
-                        (at: Ticks(20), mul: 0.3),
+                        (at: Ticks(20), mul: 0.3),,
```

Checker output:

```text
error[schema/type]: Expected opening `(` for struct `Key`.
  --> corpus/errors/e09-comma-misuse.ron:49:51
   |
49 |                         (at: Ticks(20), mul: 0.3),,
   |                                                   ^
  = path: emitters.bloom.modifiers[2].keys[3]
```

### e09 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -44 +44 @@
-      (at = 20t, mul = 0.3),
+      (at = 20t, mul = 0.3),,
```

Checker output:

```text
error[parse/comma]: Unexpected `,` in list `keys`: expected a value or `]`.
  --> corpus/errors/e09-comma-misuse.sigil:44:29
   |
44 |       (at = 20t, mul = 0.3),,
   |                             ^
  = path: emitters.bloom.modifiers[2].keys
  = help: Remove the extra comma; a single trailing comma is allowed.
```


## e10: Falscher Versions-Header (base pattern 03-subemitter-cascade)

### e10 RON

Injected change (unified diff against the correct file):

```diff
@@ -10 +10 @@
-    version: 1,
+    version: 2,
```

Checker output:

```text
error[validate/version]: Unsupported Sigil version 2; this compiler reads version 1.
  --> corpus/errors/e10-wrong-version.ron:10:14
   |
10 |     version: 2,
   |              ^
  = path: version
  = help: Set `version: 1,` or migrate the file with a newer `sigilc`.
```

### e10 sigil 1

Injected change (unified diff against the correct file):

```diff
@@ -1 +1 @@
-sigil 1
+sigil 2
```

Checker output:

```text
error[parse/version]: Unsupported Sigil version 2; this compiler reads `sigil 1`.
  --> corpus/errors/e10-wrong-version.sigil:1:7
  |
1 | sigil 2
  |       ^
  = path: version
  = help: Change the header to `sigil 1` or migrate the file with a newer `sigilc`.
```

