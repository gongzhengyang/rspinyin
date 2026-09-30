# Build and verify rspinyin on Ubuntu 22.04 LTS.
#
# This is the oldest userland in the platform baseline, and the slice exists for the
# floor it sets: glibc 2.35, GCC 11, and an Fcitx5 old enough that the addon directory
# is resolved by the multiarch triplet rather than from a libdir the package reports
# directly.
#
# One thing this slice deliberately does *not* prove: that a jammy session loads the
# plugin. The Fcitx5 shipped by 22.04 is 5.0.x, below the `core:5.1.0` floor the addon
# descriptors declare, and Fcitx5 suppresses an addon whose required dependency is
# unmet. The build, the tests and the installed layout are what this image is for; the
# runtime verdict for 22.04 is recorded in the specification, not re-decided here.
#
# The base image is a build argument so its digest can be pinned without touching the
# package list: replace `ubuntu:22.04` with `ubuntu:22.04@sha256:<digest>`. The
# workflow records the digest the tag resolved to on every run, which is the value to
# paste here.
#
# This image carries the distribution's dependencies only; the workflow mounts `just`,
# cargo-nextest and the Rust toolchain from the runner so the whole matrix runs one
# pinned version of each.

ARG BASE_IMAGE=ubuntu:22.04
FROM ${BASE_IMAGE}

# See the 24.04 image for the per-package reasoning; the list is the same because the
# two releases package the same things under the same names.
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
        binutils \
        build-essential \
        ca-certificates \
        findutils \
        fonts-dejavu-core \
        git \
        gzip \
        libfcitx5config-dev \
        libfcitx5core-dev \
        libfcitx5utils-dev \
        libfontconfig1-dev \
        pkg-config \
        tar \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /work
