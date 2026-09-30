# Build and verify rspinyin on Fedora 41.
#
# Fedora is the slice that puts the addon somewhere Debian and Ubuntu never do:
# Fcitx5 addons live under /usr/lib64/fcitx5, and `xtask install` has to derive that
# from the target system's pkg-config rather than assume a libdir. A hardcoded path
# installs the plugin where Fcitx5 never looks for it, which is a failure that shows
# up as "the input method is not in the list" and nothing else.
#
# Fedora 41 is past its end of life, so its packages have moved off the mirrors into
# the release archive. A released Fedora image's repository files still point at the
# mirrors, which is why the install below falls back to the archive when the first
# attempt fails. A release that is still supported does not need that fallback -- check
# the release's status before moving this image forward, and drop the fallback for one
# that is still current.
#
# The base image is a build argument so its digest can be pinned without touching the
# package list: replace `fedora:41` with `fedora:41@sha256:<digest>`. The workflow
# records the digest the tag resolved to on every run, which is the value to paste here.
#
# This image carries the distribution's dependencies only; the workflow mounts `just`,
# cargo-nextest and the Rust toolchain from the runner so the whole matrix runs one
# pinned version of each.

ARG BASE_IMAGE=fedora:41
FROM ${BASE_IMAGE}

# fcitx5-devel: publishes Fcitx5Core, Fcitx5Utils and Fcitx5Config to pkg-config and
#   supplies the C++ headers the glue compiles against. Without it both addon crates'
#   build scripts abort with `platform/fcitx5/dev-missing`.
# gcc-c++: the compiler the `cc` crate drives for the C++ glue, plus the linker.
# pkgconf-pkg-config: the `pkg-config` binary build.rs and `xtask install` query the
#   Fcitx5 packages through.
# fontconfig-devel: the software renderer resolves system fonts, and dejavu-sans-fonts
#   gives a test that draws text something to resolve.
# binutils: `strip` and `nm`; an install strips each library with --strip-unneeded and
#   then re-reads its dynamic symbol table to confirm the addon factory survived.
# diffutils: the install round trip diffs the staging tree before and after an
#   uninstall, which is the assertion that makes the uninstall reversible.
# findutils, tar, gzip: the install round trip snapshots a staging tree with
#   `find -printf`, and the packager writes a tarball.
# git: the runner image has it, and a container that lacks a tool the runner has fails
#   for a reason that has nothing to do with the distribution under test.
RUN set -eu; \
    packages="binutils ca-certificates dejavu-sans-fonts diffutils fcitx5-devel \
        findutils fontconfig-devel gcc-c++ git gzip pkgconf-pkg-config tar"; \
    if ! dnf install --assumeyes --setopt=install_weak_deps=False ${packages}; then \
        echo "fedora: the mirrors did not serve this release; retrying against the archive." >&2; \
        echo "fedora: an end-of-life Fedora is served from dl.fedoraproject.org/pub/archive/fedora." >&2; \
        sed --in-place \
            --expression 's|^metalink=|#metalink=|g' \
            --expression 's|^#baseurl=http://download.example/pub/fedora/linux|baseurl=https://dl.fedoraproject.org/pub/archive/fedora|g' \
            /etc/yum.repos.d/fedora*.repo; \
        dnf install --assumeyes --setopt=install_weak_deps=False ${packages}; \
    fi; \
    dnf clean all

WORKDIR /work
