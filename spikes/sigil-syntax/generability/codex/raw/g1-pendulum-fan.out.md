```ron
#![enable(implicit_some)]
Sigil(
    version: 1,
    meta: Meta(
        name: "Pendulum Fan",
        difficulty: ["normal"],
        density: 90,
    ),
    bullets: [
        Bullet(
            name: "leaf",
            silhouette: "leaf",
            palette: "enemy.jade",
            glow: 0.4,
            radius: Units(0.15),
            damage: 1,
            flags: [Grazeable],
        ),
    ],
    emitters: [
        Emitter(
            name: "pendulum",
            bullet: "leaf",
            repeat: Times(12),
            interval: Ticks(10),
            speed: UnitsPerTick(0.1),
            block: Fan(
                count: 7,
                spread: Deg(90.0),
                center: Deg(270.0),
            ),
            modifiers: [
                Rotate(
                    rate: DegPerTick(1.5),
                ),
                SineOffset(
                    amplitude: Units(0.5),
                    period: Ticks(60),
                    phase: Deg(0.0),
                ),
            ],
        ),
    ],
)
```

```sigil
sigil 1

meta {
  name = "Pendulum Fan"
  difficulty = [normal]
  density = 90
}

bullet leaf {
  silhouette = leaf
  palette = enemy.jade
  glow = 0.4
  radius = 0.15u
  damage = 1
  flags = [grazeable]
}

emitter pendulum {
  bullet = leaf
  repeat = 12
  interval = 10t
  speed = 0.1u/t
  block fan {
    count = 7
    spread = 90deg
    center = 270deg
  }
  modifier rotate {
    rate = 1.5deg/t
  }
  modifier sine_offset {
    amplitude = 0.5u
    period = 60t
    phase = 0deg
  }
}
```