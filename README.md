<p align="center">
  <img src="static/logo.svg" alt="redknife logo" width="300">
</p>

<p align="center">
  <strong>A Swiss Army knife for offensive security.</strong>
</p>

## Features

- **HTTP Callback Server**: Receive and log incoming HTTP requests.
- **HTTP Interceptor**: Intercept, inspect, modify, and forward HTTP requests.

## Install

**Get compiled artifact from releases**

Download the binary from [GitHub Releases](https://github.com/yukibtc/redknife/releases).

Optionally, before renaming the binary, verify its signature and checksum. Download `SHA256SUMS`
and `SHA256SUMS.asc` from the same release into the same directory and import the signing key
listed in [SECURITY.md](SECURITY.md):

```bash
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
```

Rename the binary to `rk` and run it:

```bash
chmod +x rk
./rk --help
```

Optionally, install `rk` system-wide to run it from any directory (requires `sudo`):

```bash
sudo install -m 755 rk /usr/local/bin/rk
```

**Install using cargo from git**

```bash
cargo install redknife --git https://github.com/yukibtc/redknife
```

**Install using cargo from crates.io**

```bash
cargo install redknife
```

## Usage

Show the available commands and general options:

```bash
rk --help
```

## Changelog

All notable changes are documented in the [CHANGELOG.md](CHANGELOG.md).

## Disclaimer

redknife is intended for educational purposes and authorized security testing only. Use it only on systems you own or have explicit permission to test.

## License

This project is distributed under the MIT software license – see the [LICENSE](LICENSE) file for details
