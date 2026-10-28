# Changelog

<!-- All notable changes to this project will be documented in this file. -->

<!-- The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), -->
<!-- and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). -->

<!-- Template

## Unreleased

### Breaking changes

### Changed

### Added

### Fixed

### Removed

### Deprecated

### Performance

### Security

-->

## Unreleased

### Breaking change

- Rename `rk http repeat` to `rk http send`
- Drop TUIs and rework `rk http proxy` and `rk http send` commands

### Added

- Add HTTP proxy request dumps
- Add HTTP proxy rules
- Add `rk codec jwt decode` and `rk codec jwt encode`
- Add `rk codec url decode` and `rk codec url encode` 
- Add `rk dns query`
- Add `rk ftp serve`
- Add `rk http serve`
- Add `rk neigh monitor`
- Add `rk neigh resolve`
- Add `rk neigh scan`
- Add `rk neigh spoof`

## 0.3.0 - 2026/10/16

### Breaking change

- Rename `rk http intercept` to `rk http proxy`
- Open HTTP Proxy with interception disabled by default

### Added

- Add LDAP Server
- TUI for HTTP Repeater
- HTTP Intercept to Repeater functionally
- HTTP Proxy history
- Automatic HTTP request and response unpacking (gzip, deflate, Brotli, and zstd)

## 0.2.0 - 2026/10/12

### Added

- Add HTTP Repeater

## 0.1.0 - 2026/10/11

First experimental release.
