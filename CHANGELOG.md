# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Changed

- Use log instead of tracing for universal error logging

## [v0.4.0] - 2022-01-04

### Changed

- Increase default max_attempts count by one
- BREAKING: Refactor project as a workspace, where integrations are separate crates

## [v0.3.0] - 2021-10-05

### Changed

- Celery `send_task` returns the full `AsyncResult` rather than only the task ID
- Add shorthand `CeleryRoundRobin` type

## [v0.2.0] - 2021-09-30

### Changed

- Upgrade to Tokio 1

### Fixed

- Fix [docs.rs](https://docs.rs/tourniquet) metadata to properly show feature flags

## [v0.1.1] - 2021-01-26

### Added

- [celery](https://github.com/rusty-celery/rusty-celery) integration

## [v0.1.0] - 2021-01-26

### Added

- Initial release with round-robin system

[v0.1.0]: https://github.com/Tuetuopay/tourniquet/releases/tag/v0.1.0
[v0.1.1]: https://github.com/Tuetuopay/tourniquet/releases/tag/v0.1.1
[v0.2.0]: https://github.com/Tuetuopay/tourniquet/releases/tag/v0.2.0
[v0.3.0]: https://github.com/Tuetuopay/tourniquet/releases/tag/v0.3.0
[v0.4.0]: https://github.com/Tuetuopay/tourniquet/releases/tag/v0.4.0
