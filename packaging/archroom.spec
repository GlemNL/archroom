# RPM spec for Fedora (and other rpm-based distros with LibRaw/gexiv2 packages).
# Built in CI from the release source tarball:
#   rpmbuild -ba packaging/archroom.spec
%global debug_package %{nil}

Name:           archroom
Version:        0.1.0
Release:        1%{?dist}
Summary:        Photo catalog and raw developer with a Lightroom-style workflow
License:        GPL-3.0-or-later
URL:            https://github.com/GlemNL/archroom
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust >= 1.85
BuildRequires:  gcc
BuildRequires:  clang
BuildRequires:  clang-devel
BuildRequires:  pkgconfig
BuildRequires:  LibRaw-devel >= 0.20
BuildRequires:  lcms2-devel
BuildRequires:  pkgconfig(gexiv2)
Requires:       vulkan-loader
Recommends:     mesa-vulkan-drivers
Recommends:     xdg-desktop-portal

%description
Archroom is a Linux-native, non-destructive photo library and raw developer:
catalog thousands of photos, cull them from the keyboard, and develop raws on
the GPU without ever touching your originals.

%prep
%autosetup

%build
cargo build --release --locked -p archroom-app -p archroom-cli

%install
install -Dm755 target/release/archroom %{buildroot}%{_bindir}/archroom
install -Dm755 target/release/archroom-cli %{buildroot}%{_bindir}/archroom-cli
install -Dm644 packaging/archroom.desktop \
    %{buildroot}%{_datadir}/applications/archroom.desktop
install -Dm644 assets/archroom.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/archroom.svg

%files
%license LICENSE
%doc README.md
%{_bindir}/archroom
%{_bindir}/archroom-cli
%{_datadir}/applications/archroom.desktop
%{_datadir}/icons/hicolor/scalable/apps/archroom.svg

%changelog
* Wed Sep 30 2026 Clément L <a.kenbari@gmail.com> - 0.1.0-1
- Initial release
