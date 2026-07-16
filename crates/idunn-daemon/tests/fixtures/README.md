# Bifrost release-authority wire fixture

`bifrost-release-authority-v1.payload.hex` is the exact MessagePack payload
emitted on 2026-07-16 by Bifrost's TypeScript CultCache writer
`tools/bifrost-repository-release-authority.mjs` for `GameCult/Epiphany`,
`refs/heads/main`, commit
`0123456789abcdef0123456789abcdef01234567`.

It is checked in because the TypeScript runtime emits a camelCase MessagePack
map while Rust `DatabaseEntry` derives tuple payloads. Idunn must prove it can
decode the producer's real wire shape rather than round-tripping a fixture it
serialized itself.
