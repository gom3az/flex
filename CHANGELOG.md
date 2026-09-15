# Changelog

All notable changes to `flex-core` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-09-15

First release as a standalone crate. Extracted from the `flex` workspace in
`gom3az/local-dot-files`, where it was the `flex-core` member beside
`flex-rice` (a Hyprland rice's providers, `flex` binary and shell wrappers).

Source is unchanged apart from this manifest (metadata inlined instead of
inherited from a workspace root) and the `repository` URL. `flex-rice` now
depends on this crate by git tag, so the one-way dependency established by the
split holds across the repository boundary.
