# Build and verify rspinyin on Fedora 40.
#
# Fedora 40 is the floor of the "Fedora 40+" row in the platform baseline, and the
# reason that row is a range rather than a version: what the slice pins is the addon
# directory (/usr/lib64/fcitx5) and the packaging generation, not a Fedora point
# release. Fedora 41 covers the other end of the range.
#
# Fedora 40 is past its end of life, so its packages have moved off the mirrors into
# the release archive. A released Fedora image's repository files still point at the
# mirrors, which is why the install below falls back to the archive when the first
# attempt fails. The archived tree is frozen at the release's last update, which is
# what makes this slice reproducible rather than merely old.
#
# The base image is a build argument so its digest can be pinned without touching the
# package list: replace `fedora:40` with `fedora:40@sha256:<digest>`. The workflow
# records the digest the tag resolved to on every run, which is the value to paste here.
#
# This image carries the distribution's dependencies only; the workflow mounts `just`,
# cargo-nextest and the Rust toolchain from the runner so the whole matrix runs one
# pinned version of each.

ARG BASE_IMAGE=fedora:40
FROM ${BASE_IMAGE}

# The package list and its per-package reasoning are the same as the Fedora 41 image:
# both releases package the same things under the same names. See that file for why
# each one is here.
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
