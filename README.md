# mechbot

A bot for the Builders Buildathon "Mech Royale" track. It speaks the `mechfight/1`
protocol: JSON lines on stdin and stdout, one action per observation.

## Layout

- `src/main.rs`: the stdin/stdout loop. Answers only the newest queued observation and
  never lets a panic escape.
- `src/world.rs`: loads the map (`world.json`, `terrain.i16`) into a 1 m height grid with
  line-of-sight and walkability queries.
- `src/nav.rs`: A* path search over that grid.
- `src/bot.rs`: the decision logic. It tells house-squad practice matches from
  free-for-alls (mechs dying that it did not kill) and plays the two differently.
- `src/practice.rs`: behaviour used only in confirmed practice matches.
- `src/cover.rs`: getting out of a laser's or a lightning carrier's line of fire.
- `src/salts.rs`: practice: per-match-group RNG salts selected by submission testing. On the
  first observation the bot hashes its slot and the first next-circle centre into one of 8
  groups and seeds its random choices (strafe timing, wander points) with that group's salt.

## Build and check

```sh
docker build -t my-bot .
python3 check/check.py --map-dir /arena -- docker run --rm -i --network none --cpus 1 --memory 512m --read-only --tmpfs /tmp:rw,size=64m --user 65534:65534 -v "$PWD/arena:/arena:ro" my-bot
```

The second command runs from the organizers' kit folder. The workflow in
`.github/workflows/check.yml` does the same on every push.

## Sources

- Written during the event with [Claude Code](https://claude.com/claude-code).
- Libraries: the Rust standard library and [`serde_json`](https://github.com/serde-rs/json)
  (MIT or Apache-2.0) with its dependencies, pinned in `Cargo.lock`.
- The protocol, map, and conformance check come from the organizers' kit and are not
  included here.

## License

MIT. See `LICENSE`.
