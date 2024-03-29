# Matcha

A peaceful package manager.

## Rationale

Frustrated by the performance of brew, and the user-hostility of nix, I set out
to build my own package manager.

A list of highlights:

- Fast. Using Rust + tokio means we can run many operations in parallel. All
  operations touching the disk or the network are asynchronous and parallelised.
  When installing several packages, we install all of them in parallel. The
  backing store is a SQLite database.
- Single binary. No install step.
- Pinnable versions, to arbitrary degrees. When you install a package, it notes
  which version you specified. No version means it'll always update to the
  latest. Install `package@1` and it won't upgrade to `2` without you telling it
  to do so. Same with `1.5`, it'll update patch versions, but not minor versions.
- Abitrary versions. Different versions are different packages. No more
  `mysql@5` package that needs manual curating. Short of yanking releases,
  versions are meant to be append-only.
- Workspaces. Need different versions of a package in different contexts? Create
  a workspace. `$PATH` gets pached in a `workspace shell`, so that a workspace
  acts as a layer of package overrides. Workspaces are stackable, and "globally"
  installed packages are just another workspace.
- Local registries. Want to manage a software that hasn't been packaged yet?
  Just add a registry TOML file from your disk, and it'll work just the same.

## Usage

Basic usage:

```sh
# Add some registries (where packages come from)
matcha registry add https://example.invalid/registry
matcha registry add ~/custom_packages.toml

# Install a package
matcha package install ripgrep
# Install a specific version (which is then pinned)
matcha package install jq@1.7.1

# Update all packages
matcha package update

# Remove a package
matcha package remove jq

# Create a workspace, add a package to it, and open a workspace shell
matcha workspace add rails-2.7
matcha package install --workspace rails-2.7 ruby@2.7
matcha workspace shell rails-2.7
```

All commands and flags are documented, and should be fairly intuitive. Most
commands also have shorter aliases.

This is the full command tree:

```
matcha
├─help [command ..]
├─package
│ ├─install <packages ..>
│ ├─update  [packages ..]
│ ├─remove  <packages ..>
│ ├─list
│ ├─show    <package>
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

### Word of Caution

This is software is roughly in alpha state. It might try to wipe your disk. Use
at your own peril.

If investigating issues, setting `MATCHA_LOG=info` (or event `debug`) will
provide additional info.

## How it Works

First a few core concepts:
- A `registry` is either a file or URL that serves a manifest.
- A `manifest` is just a collection of packages.
- A `package` can have different shapes, but all include a name and a version.
  In the context of a manifest, packages have more metadata, including a source
  and build instructions.
- A `workspace` is an environment that uses a selected subset of the installed
  packages.

Packages are always identified by a name and a version, from which follows that
different versions result in different packages. To avoid naming conflicts,
package names have to be unique within workspaces, as we would otherwise not
know which binary to use.

Internally the manifest data is cached in SQLite, and refreshed automatically
when appropriate.

Operating on packages from a user's perspective always means manipulating the
packages included in the current workspace. The default workspace is called
`global`. Within a `workspace shell`, the currently active workspace is
modified.

As a complete example, let's look at installing a package:

1. The user requests a package, with at least a name, and maybe a partially or
   fully pinned version.
1. We ensure that all registries are current by maybe re-fetching some of them.
1. We compare the user's request against the packages we know how to build and
   find a suitable version.
1. We check if that package, now with a fully qualified version, is already
   installed. If so, we just link it into the current workspace, and are done.
1. Otherwise we fetch the package source, perform the build instructions, and
   install the package into the package directory. Then we can link it from
   there into the current workspace.

This system is somewhat similar to what Nix does, in that installing a package
is decoupled from the ability to use it. Reusing an already existing package in
a different workspace is effectively free, as we just create a symlink and a
database record.

## Building

```sh
cargo build --release
```

Note that tests require [cargo nextest](https://nexte.st/).

## Packaging

A minimal example manifest looks like this:

```toml
schema_version = 1
name = "my-registry"

[[packages]]
name = "my-package"
version = "1.0"
source = "https://my-website.invalid/my-package/1.0/download.zip"
build = """
unzip $MATCHA_SOURCE
cd my-package
cargo build --release
mkdir $MATCHA_OUTPUT/bin
cp target/release/my-package $MATCHA_OUTPUT/bin/my-package
"""
```

A full example would be:

```toml
schema_version = 1
name = "my-registry"
description = "An example manifest"

[[packages]]
name = "test-package"
version = "0.1.0"
description = "A test package"
homepage = "https://example.invalid/test-package"
license = "MIT"
source = "https://example.invalid/test-package-0.1.0.zip"
build = """
unzip $MATCHA_SOURCE
cd test-package
cargo build --release
mkdir $MATCHA_OUTPUT/bin
cp target/release/test-package $MATCHA_OUTPUT/bin/test-package
"""

[[packages]]
name = "test-package"
version = "0.1.1"
description = "A test package"
homepage = "https://example.invalid/test-package"
license = "MIT"
artifacts = ["bin/my-package"]
source = "https://example.invalid/test-package-0.1.1.zip"
build = """
unzip $MATCHA_SOURCE
cd test-package
cargo build --release
mkdir $MATCHA_OUTPUT/bin
cp target/release/test-package $MATCHA_OUTPUT/bin/test-package
"""
```

Anything inside `$MATCHA_OUTPUT/bin` will get placed in a workspace's `bin`
directory, which then goes into `$PATH` for workspace shells.

## Future Plans

- Build dependencies, i.e. packages that need to be available to build another
  package. This should be fairly simple by making up a temporary workspace for
  the build process, which is populated with those packages.
- Runtime dependencies, e.g. Python for yt-dlp. This just means installing one
  package pulls in a few others as well. Need to add dependency version
  resolution for that. I don't want to do the thing Nix does and require each
  package to be patched so that we can pass in a specific dependency for each
  package, so I probably won't be able to avoid version conflicts altogether,
  though workspaces could act as workarounds.
- Bundles, as in dumping out the currently installed packages in a workspace,
  and loading them up into a workspace on a different machine. This can already
  be scripted with `package list`, but why not support it directly.
- Multi-version packages to make packaging easier. Instead of having one package
  per version in a manifest, allow a package to define a list of versions that
  are all identical except for the `source`. To that end include some templating
  functionality as well.
- Potentially adding support for JSON/YAML registries, I appreciate that TOML is
  not everyone's cup of tea.
- Download hashes, to verify file integrity.
- Non-executable-binary package artifacts. Right now we only place files from
  `$MATCHA_OUTPUT/bin` in a directory that gets added to `$PATH`, but we will
  want to produce other artifacts such as man pages, config files, etc.
- License-aware SBOMs, and dependency trees.
