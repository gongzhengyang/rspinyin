# RPM spec for rspinyin.
#
# Fedora installs Fcitx5 addons under %{_libdir}/fcitx5, which is /usr/lib64 on
# x86_64 and aarch64 and /usr/lib on 32-bit. The spec does not hardcode any of
# them: `xtask install` asks the target system's pkg-config, and %{_libdir} is
# used only to tell rpm which files to expect in the manifest.

# Both shared objects are addon plugins, not libraries anything links against,
# and nothing in the package is a debug-information carrier: the release profile
# is LTO with one codegen unit, and the installer strips each library at
# packaging time. A -debuginfo subpackage would therefore be empty, and
# find-debuginfo would run over files it has nothing to extract from.
%global debug_package %{nil}

# The libraries are stripped by `xtask install` with `strip --strip-unneeded`,
# which keeps the dynamic symbol table, and the symbol Fcitx5 resolves with
# `dlsym` is read back from the build root in %check below. rpm's own brp-strip
# is turned off so that nothing re-strips them afterwards with a policy this
# project does not control: brp-strip's arguments are the distribution's, and a
# `--strip-all` there would take `fcitx_addon_factory_instance` out of the
# dynamic symbol table and leave a library Fcitx5 loads and finds no addon in.
# There is nothing for it to do in any case -- the files are already stripped.
%global __brp_strip %{nil}

Name:           rspinyin
Version:        0.1.0
Release:        1%{?dist}
Summary:        Offline-first Chinese pinyin input method for Fcitx5

# The project grants a choice between the two licences, so the expression is
# `OR`, not `AND`: Fedora's SPDX guidance uses OR when the recipient may pick,
# and AND when different parts of the package are under different licences and
# all of them apply. Both texts ship in %license below. Slint, which is
# statically linked into the UI addon, is deliberately absent from this field:
# its identifier is a `LicenseRef-`, and Fedora accepts only Fedora-recognised
# identifiers here. Its licence text ships as a %license file instead.
License:        MIT OR Apache-2.0
URL:            https://github.com/gongzhengyang/rspinyin
Source0:        %{url}/archive/v%{version}/rspinyin-%{version}.tar.gz

BuildRequires:  binutils
BuildRequires:  cargo
BuildRequires:  curl
BuildRequires:  fcitx5-devel
BuildRequires:  gcc-c++
BuildRequires:  pkgconf-pkg-config
BuildRequires:  python3
# Edition 2024 needs rustc 1.85, and Cargo refuses to build a member whose
# rust-version is above the toolchain's. Stating the bound makes an older
# archive fail at dependency resolution, naming the version, instead of minutes
# into a compile. Fedora 41's archive Rust is 1.82, so the floor is only met
# from Fedora 42 onward unless a newer toolchain is supplied separately.
BuildRequires:  rust >= 1.85
BuildRequires:  unzip
Requires:       fcitx5 >= 5.1.0

# The plugin is an in-process addon: it links against the same libFcitx5Core the
# host process already loaded, so the automatic library dependency generator
# must record it. Without this the package installs and then fails to load.
AutoReqProv:    yes

%description
rspinyin is an in-process Fcitx5 addon that draws its own candidate window
instead of delegating to ClassicUI. It decodes full-pinyin input with a k-best
Viterbi search over a compiled FST dictionary, learns from the user's own word
frequency, and never opens a network socket: there is no cloud input, no
dictionary update channel and no telemetry.

The candidate window is rendered by a software rasteriser into a shared-memory
buffer, so it needs no GPU and no display-server-specific toolkit.

The package installs two shared objects, not one. Fcitx5 picks the active user
interface from the addons it discovered with Category=UI, and an addon belongs
to exactly one category, so the decoding engine (Category=InputMethod) cannot
also be the candidate window.

%prep
%setup -q -n rspinyin-%{version}

%build
# The raw upstream dictionary sources are not committed; data/sources.toml pins
# them by digest instead. A tree that has never fetched them cannot compile the
# dictionary this package ships, so fetch the missing ones -- and only then. A
# packager who has already run data/fetch.sh, or a source archive that carries
# them, builds without touching the network at all.
for source in pinyin-data jieba-dict unihan; do
  if [ ! -f "data/raw/${source}.tsv" ]; then
    bash data/fetch.sh
    break
  fi
done

# SOURCE_DATE_EPOCH keeps the C++ glue's debug info and any __DATE__/__TIME__
# expansion stable across rebuilds. rpmbuild sets it from the changelog on the
# versions that support reproducible builds; the fallback is the epoch the rest
# of the project's builds use, so a build is never stamped with the clock.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1790899200}"
export CXXFLAGS="-ffile-prefix-map=$PWD=. -fdebug-prefix-map=$PWD=. ${CXXFLAGS:-}"

# Both addons, not just the engine: ADR-0003 splits the plugin into an
# `InputMethod` addon and a `UI` addon, and the installer refuses to run when
# either library is missing rather than shipping half an input method.
cargo build --release --locked -p ime-fcitx5 --features fcitx5-host
cargo build --release --locked -p ime-ui-addon --features fcitx5-host
cargo run --release --locked -p xtask -- dictc

%install
rm -rf %{buildroot}
DESTDIR=%{buildroot} PREFIX=%{_prefix} \
  bash packaging/install.sh --no-sudo --skip-build
# The installer writes an install manifest so that its own uninstall can put
# back whatever a previous installation displaced. Every path in it is prefixed
# with %{buildroot}, so shipping it would put a record of this build machine's
# directory layout on the user's disk. Removal here belongs to rpm, not to the
# installer.
find %{buildroot} -name install-manifest.json -delete

%check
# The test suite runs in CI on every commit and needs cargo-nextest, which is
# not in Fedora's archive. What this package can assert cheaply, and what
# nothing else in the chain asserts, is that the files about to be shipped still
# work: the installer stripped both libraries with `strip --strip-unneeded` and
# re-read their dynamic symbol tables, and this reads them again from the build
# root -- the tree the package is made from, not the one the installer staged.
for so in %{buildroot}%{_libdir}/fcitx5/librspinyin.so \
          %{buildroot}%{_libdir}/fcitx5/librspinyin_ui.so; do
  test -f "$so" \
    || { echo "dist/verify/artifact-missing: $so" >&2; exit 1; }
  nm -D --defined-only "$so" | grep -q fcitx_addon_factory_instance \
    || { echo "dist/verify/factory-symbol-missing: $so" >&2; exit 1; }
  if readelf -SW "$so" | grep -q '\.symtab'; then
    echo "dist/verify/not-stripped: $so left the build root unstripped" >&2
    exit 1
  fi
done
test -s %{buildroot}%{_datadir}/rspinyin/base.dict \
  || { echo "dist/verify/artifact-missing: base.dict" >&2; exit 1; }

%files
# Both texts of the project's own dual licence, because the License field above
# names both and the recipient may pick either.
%license LICENSE-MIT
%license LICENSE-APACHE
# Slint's licence text. It cannot be named in the License field (see above), so
# the text itself is what carries the obligation and the attribution.
%license LICENSES/LicenseRef-Slint-Royalty-free-2.0.md
%doc docs/dev/NOTICE
%{_libdir}/fcitx5/librspinyin.so
%{_libdir}/fcitx5/librspinyin_ui.so
%{_datadir}/fcitx5/addon/rspinyin.conf
%{_datadir}/fcitx5/addon/rspinyin-ui.conf
%{_datadir}/fcitx5/inputmethod/rspinyin.conf
%{_datadir}/rspinyin/base.dict
%{_datadir}/icons/hicolor/48x48/apps/fcitx-rspinyin.png
%{_datadir}/icons/hicolor/scalable/apps/fcitx-rspinyin.svg
%{_metainfodir}/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml

%changelog
* Tue Sep 29 2026 rspinyin contributors <rspinyin@localhost> - 0.1.0-1
- Initial package.
