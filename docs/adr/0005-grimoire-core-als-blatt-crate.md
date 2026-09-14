# ADR-0005: `grimoire_core` als abhängigkeitsfreie Blatt-Crate

- **Status:** Akzeptiert
- **Datum:** 2026-09-14
- **Entscheider:** Lupus Malus Deviant (PO), vorbereitet durch Claude
- **Bezug:** Spiel-Repo `docs/prd/0000-index-fiends-n-patrons.md` §4 (Crate-Map), `docs/adr/0005-voll-deterministische-simulation.md`, [ADR-0004](0004-deterministische-gleitkommaarithmetik.md)

## Kontext

Die ursprüngliche Crate-Map (PRD-0000 §4) kennt kein gemeinsames Fundament unterhalb von
`grimoire_ecs` und `grimoire_sim`. Beim Schnitt der P0-Verträge zeigte sich, dass zwei Bausteine von
mehreren Simulations-Crates gebraucht werden:

- **Stabiles Hashing** (`StableHash`, `StableHasher`): Jede Komponente und Ressource im ECS muss
  hashbar sein, `grimoire_sim` bildet daraus den Zustands-Hash für Golden Master und Replays,
  später hashen `grimoire_collide` und `grimoire_sigil` ihre Zustände.
- **Deterministische Mathematik** (`dmath`, `Vec2`): Bewegung, Kollision und Pattern-Interpreter
  dürfen keine plattformabhängigen `std`-Transzendentalfunktionen nutzen.

`grimoire_sim` hängt von `grimoire_ecs` ab. Läge der Hash-Vertrag in `grimoire_sim`, entstünde ein
Abhängigkeitszyklus.

## Anforderungen

- Kein Zyklus im Crate-Graphen; Abhängigkeiten zeigen nur nach unten.
- Determinismus-Primitiven an genau einer Stelle, mit Golden-Tests plattformübergreifend eingefroren.
- Minimale Abhängigkeiten (Kompilierzeit, Portabilität auf Mobile).
- Kein Zwang, Mathe über das ECS zu beziehen.

## Optionen

1. **Primitiven in `grimoire_ecs`** — zyklusfrei, aber Mathematik gehört fachlich nicht ins ECS;
   `grimoire_collide` und `grimoire_sigil` müssten für `Vec2` am ECS hängen.
2. **Hash-Vertrag in `grimoire_ecs`, Mathematik in `grimoire_sim`** — verstreut zusammengehörige
   Determinismus-Regeln auf zwei Crates, Golden-Tests wären doppelt zu pflegen.
3. **Fremdbibliotheken (`glam` + `std::hash`)** — `std::hash`-Ausgaben sind über Rust-Versionen
   nicht garantiert stabil; `glam` wählt je Plattform SIMD- oder Skalarpfade und bietet
   Transzendentalfunktionen über `std` an — beides untergräbt die Determinismus-Garantie.
4. **Eigene Blatt-Crate `grimoire_core` (gewählt)** — enthält ausschließlich deterministische,
   plattformunabhängige Primitiven; einzige Abhängigkeit ist `libm`.

## Entscheidung

Option 4. `grimoire_core` ist die unterste Engine-Crate und darf von keiner anderen
`grimoire_*`-Crate abhängen. Sie enthält in P0 das stabile Hashing (Algorithmus v1) und die
deterministische Mathematik. Die Determinismus-Lint-Konfiguration (`clippy.toml`) gilt auch hier.

Nicht in `grimoire_core` gehören: Rendering-Typen (der Renderer bleibt bei `[f32; N]`-Arrays und
hängt nicht von `grimoire_core` ab), Plattform- oder Zeitfunktionen, ECS-Konzepte.

## Konsequenzen

- (+) Zyklusfreier Graph; `grimoire_ecs`, `grimoire_sim`, `grimoire_collide` und `grimoire_sigil`
  teilen dieselben Primitiven und dieselben Golden-Tests.
- (+) Algorithmuswechsel beim Hashing sind an einer Stelle versioniert (`ALGORITHM_VERSION`).
- (−) Die Crate-Map in PRD-0000 §4 und das Architekturdiagramm in PRD-0002 werden um
  `grimoire_core` ergänzt.
- (−) Gefahr einer „Sammelkiste": Neue Inhalte kommen nur hinein, wenn mindestens zwei
  Simulations-Crates sie brauchen und sie deterministisch sowie plattformunabhängig sind.
