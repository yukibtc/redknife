<p align="center">
  <img src="static/logo.svg" alt="redknife logo" width="300">
</p>

<p align="center">
  <strong>A Swiss Army knife for offensive security.</strong>
</p>

<p align="center">Red teaming and pentesting tools bundled into a single binary — one tool, many blades.</p>

## Features

- **DNS**
  - **DNS Query**: Query DNS servers.

- **FTP**
  - **FTP Server**: A minimal FTP server.

- **HTTP**
  - **HTTP Callback Server**: Receive and log incoming HTTP requests.
  - **HTTP Proxy**: Inspect, intercept, modify, and forward/drop HTTP requests.
  - **HTTP Send**: Send and replay HTTP and HTTPS requests.
  - **HTTP Server**: Serve a directory over HTTP with optional file upload support.

- **LDAP**
  - **LDAP Server**: A minimal LDAP server.

- **Neighbor (ARP/NDP)**
  - **Neighbor Monitor**: Passively monitor neighbor traffic to discover hosts. (**NDP not available yet**)
  - **Neighbor Resolver**: Resolve IP to MAC addresses. (**NDP not available yet**)
  - **Neighbor Scanner**: Actively scan a subnet to discover hosts. (**NDP not available yet**)
  - **Neighbor Spoofing**: Poison neighbor caches. (**NDP not available yet**)

- **Utilities**
  - **JWT**: JSON Web Token encoding, decoding and signing.
  - **PowerShell**: PowerShell `-EncodedCommand` strings (base64 of UTF-16LE) encoding and decoding.
  - **URL**: Percent-encoding and decoding.
  - **XOR**: Reversible XOR transform.  

## Supported platforms

`redknife` is built and actively tested on Linux. Other platforms are not actively tested at this time and may not be fully featured.

Precompiled binaries are distributed for Linux and Windows; on other platforms, `redknife` must be built from source. 
Below you can find instructions for both precompiled binaries and installing from source.

## Install

**Get compiled artifact from releases**

Download the archive for your architecture from [GitHub Releases](https://github.com/yukibtc/redknife/releases).

Optionally, before extracting the archive, verify its signature and checksum. 
Download `SHA256SUMS` and `SHA256SUMS.asc` from the same release into the same directory and import the signing key listed in [SECURITY.md](SECURITY.md):

```bash
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
```

Extract the downloaded  `.tar.gz` archive and run:

```bash
./rk --help
```

On Windows, extract the `.zip` archive and run:

```powershell
.\rk.exe --help
```

Optionally, install `rk` system-wide to run it from any directory (requires `sudo`):

```bash
sudo install -m 755 rk /usr/local/bin/rk
```

or user-wide (ensure `~/.local/bin` is in your `PATH`):

```bash
install -m 755 rk ~/.local/bin/rk
```

**Install using cargo from source**

The following command requires `rustc` and `cargo`, and will build and install `redknife` directly from the git repository:

```bash
cargo install redknife --git https://github.com/yukibtc/redknife
```

## Usage

Show the available commands and general options:

```bash
rk --help
```

## Changelog

All notable changes are documented in the [CHANGELOG.md](CHANGELOG.md).

## State

redknife is still in early development! Features and APIs will change, and bugs are expected.

## Disclaimer

redknife is intended for educational purposes and authorized security testing only. Use it only on systems you own or have explicit permission to test.

## License

This project is distributed under the MIT software license – see the [LICENSE](LICENSE) file for details
