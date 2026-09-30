# Build and verify rspinyin on Arch Linux.
#
# Arch is the shortest of the three addon layouts -- /usr/lib/fcitx5, with no triplet
# and no lib64 -- and the one a hardcoded Debian-style path gets wrong most visibly.
#
# It is also the only rolling distribution in the baseline, and this image is
# deliberately less reproducible than the other four. That is the slice's value rather
# than a defect to work around: Arch is what catches breakage from a newer compiler or
# a newer Fcitx5 before a stable release does, so the image upgrades itself to whatever
# the distribution is today instead of freezing at a point release. Pin the slice by
# naming one of Arch's dated base-devel tags instead of the rolling one, and accept
# that the frozen tag stops answering that question.
#
# The base image is a build argument so its digest can be pinned without touching the
# package list: replace `archlinux:base-devel` with
# `archlinux:base-devel@sha256:<digest>`. The workflow records the digest the tag
# resolved to on every run, which is the value to paste here.
#
# This image carries the distribution's dependencies only; the workflow mounts `just`,
# cargo-nextest and the Rust toolchain from the runner so the whole matrix runs one
# pinned version of each.

ARG BASE_IMAGE=archlinux:base-devel
FROM ${BASE_IMAGE}

# The keyring is refreshed first: an image published a few weeks ago can carry a
# keyring that predates a signing key rotation, and the failure that produces is a
# signature error that reads like a corrupt package.
#
# fcitx5: Arch does not split development packages, so this one carries the headers and
#   the Fcitx5Core/Fcitx5Utils/Fcitx5Config pkg-config files the glue compiles against.
#   Without it both addon crates' build scripts abort with `platform/fcitx5/dev-missing`.
# fontconfig and ttf-dejavu: the software renderer resolves system fonts, so the build
#   needs the fontconfig headers and a test that draws text needs a font to resolve.
# git: the runner image has it, and a container that lacks a tool the runner has fails
#   for a reason that has nothing to do with the distribution under test.
# ca-certificates: cargo reaches the registry over TLS.
#
# gcc, binutils (`strip`, `nm`), pkgconf, findutils, tar and gzip are already in the
# base-devel image.
RUN pacman --sync --refresh --noconfirm archlinux-keyring \
 && pacman --sync --refresh --sysupgrade --noconfirm --needed \
        ca-certificates \
        fcitx5 \
        fontconfig \
        git \
        ttf-dejavu \
 && pacman --sync --clean --noconfirm

WORKDIR /work
