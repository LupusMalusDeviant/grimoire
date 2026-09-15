```ron
#![enable(implicit_some)]
Sigil(
    version: 1,
    meta: Meta(
        name: "Mine Field",
        difficulty: ["hard", "trap"],
        density: 200,
        patron: "ember_court",
    ),
    bullets: [
        Bullet(
            name: "mine",
            silhouette: "orb",
            palette: "enemy.crimson",
            glow: 0.8,
            radius: Units(0.4),
            damage: 2,
            flags: [Smashable, Grazeable],
            transforms: [
                Burst(
                    when: Time(Ticks(90)),
                    bullet: "splinter",
                    speed: UnitsPerTick(0.12),
                    block: Ring(
                        count: 8,
                        start: Deg(0.0),
                    ),
                ),
            ],
        ),
        Bullet(
            name: "splinter",
            silhouette: "shard",
            palette: "enemy.amber",
            glow: 0.5,
            radius: Units(0.12),
            damage: 1,
            flags: [Grazeable],
            transforms: [
                ChangeType(
                    when: Distance(Units(6.0)),
                    to: "cinder",
                ),
            ],
        ),
        Bullet(
            name: "cinder",
            silhouette: "diamond",
            palette: "enemy.amber",
            glow: 0.2,
            radius: Units(0.12),
            damage: 1,
            flags: [Grazeable],
            despawn_vfx: "ash_puff",
        ),
    ],
    emitters: [
        Emitter(
            name: "layer",
            bullet: "mine",
            delay: Ticks(30),
            repeat: Times(1),
            interval: Ticks(1),
            speed: UnitsPerTick(0.02),
            block: Line(
                count: 5,
                spacing: Units(1.2),
                direction: Deg(0.0),
            ),
        ),
    ],
)
```

```sigil
sigil 1

meta {
  name = "Mine Field"
  difficulty = [hard, trap]
  density = 200
  patron = ember_court
}

bullet mine {
  silhouette = orb
  palette = enemy.crimson
  glow = 0.8
  radius = 0.4u
  damage = 2
  flags = [smashable, grazeable]
  transform burst {
    when = time 90t
    bullet = splinter
    speed = 0.12u/t
    block ring {
      count = 8
      start = 0deg
    }
  }
}

bullet splinter {
  silhouette = shard
  palette = enemy.amber
  glow = 0.5
  radius = 0.12u
  damage = 1
  flags = [grazeable]
  transform change_type {
    when = distance 6u
    to = cinder
  }
}

bullet cinder {
  silhouette = diamond
  palette = enemy.amber
  glow = 0.2
  radius = 0.12u
  damage = 1
  flags = [grazeable]
  despawn_vfx = ash_puff
}

emitter layer {
  bullet = mine
  delay = 30t
  repeat = 1
  interval = 1t
  speed = 0.02u/t
  block line {
    count = 5
    spacing = 1.2u
    direction = 0deg
  }
}
```