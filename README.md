# Macha

A peaceful package manager.

## Rationale

- portable - single binary
- fast - almost everything is done in parallel
- pinnable versions - no surprises
- install arbitrary versions, or multiple
- SBOM-ready
    - dependency trees
    - license-aware
- sandbox shells
- intuitive - many command aliases, including single-letter ones
- informational output to stderr - so you can pipe in peace

## Manual

This is the command tree:

```
matcha
├─help
├─package
│ ├─install <packages ..>
│ ├─update  [packages ..]
│ ├─remove  <packages ..>
│ ├─list
│ └─search  <query>
├─workspace
│ ├─add     <name>
│ ├─remove  <name>
│ ├─list
│ └─shell   <name>
└─registry
  ├─add     <uri>
  ├─remove  <name>
  ├─list
  └─fetch
```

## Building

```sh
cargo build --release
```

Note that tests require [cargo nextest](https://nexte.st/).

## Packaging

A `registry` is either a file or URL that serves a `manifest`.

A `manifest` is just a collection of packages.

A minimal example manifest looks like this:

```toml
schema_version = 1
name = "my-registry"

[[packages]]
name = "my-package"
version = "1.0"
source = "https://my-website.invalid/my-package/1.0/download.zip"
build = """
unzip download.zip
cd my-package
cargo build --release
cp target/release/my-package ../bin/my-package
"""
artifacts = ["bin/my-package"]
```