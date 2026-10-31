# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2](https://github.com/abusch/rustdoc-viewer/compare/v0.1.1...v0.1.2) - 2026-10-31

### Changed

- share path, reference and parameter formatting in format.rs
- derive the syntax theme from DEFAULT_THEME and tidy render.rs
- tighten App's history, sizing and help state
- drop default_expanded and tidy page.rs comments
- clear the status inside handle_key and drop dead key arms
- carry link ids beside spans instead of tagging underline colours
- let the page builder own its sections and member rows
- name and classify item kinds in one place

### Documentation

- add CLAUDE.md

### Fixed

- decide whether a code block is Rust the way rustdoc does
- show deprecation notices in bold so they outweigh unstable ones
- list unions, statics, and attribute and derive macros on module pages
- show items under the paths rustdoc publishes them at

### Other

- Tweak ui

## [0.1.1](https://github.com/abusch/rustdoc-viewer/compare/v0.1.0...v0.1.1) - 2026-08-26

### Added

- show methods on their type's page instead of a page of their own
- theme the UI with opaline, default both themes to catppuccin mocha

## [0.1.0]

Initial release. `rdv`, a terminal browser for Rust `std` documentation:
fuzzy search over every item, rendered docs with syntax-highlighted
examples, and intra-doc link navigation.
