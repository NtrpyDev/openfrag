Name:           openfrag
Version:        1.0.0
Release:        1%{?dist}
Summary:        Local CS2 demo analysis and highlight capture dashboard

License:        GPL-3.0-only
URL:            https://github.com/NtrpyDev/openfrag
Source0:        %{url}/releases/download/v%{version}/%{name}-v%{version}-source.tar.gz
ExclusiveArch:  x86_64

%global source_sha256 9e109aa1a2205613ff0607111903a13a8497333e3b28cef1ae359b3b314531e4

BuildRequires:  cargo >= 1.96
BuildRequires:  desktop-file-utils
BuildRequires:  gcc
BuildRequires:  rust >= 1.96
BuildRequires:  systemd-rpm-macros
Requires:       bash
Requires:       ffmpeg-free
Requires:       systemd
Requires:       xdg-utils

%description
OpenFrag is a local Linux application for importing Counter-Strike 2 demos,
reviewing evidence-backed match statistics, and capturing optional highlights.
It has no account, backend, telemetry, or upload path.

%prep
printf '%s  %s\n' '%{source_sha256}' '%{SOURCE0}' | sha256sum -c -
%autosetup -n %{name}-v%{version}-source

%build
cargo build --frozen --release -p openfragd

%check
cargo test --frozen -p openfragd -- --test-threads=1
desktop-file-validate packaging/linux/io.github.ntrpydev.openfrag.desktop

%install
install -Dm0755 target/release/openfragd %{buildroot}%{_bindir}/openfragd
install -Dm0755 packaging/linux/openfrag-launch %{buildroot}%{_bindir}/openfrag-launch
install -Dm0644 packaging/linux/io.github.ntrpydev.openfrag.desktop \
    %{buildroot}%{_datadir}/applications/io.github.ntrpydev.openfrag.desktop
install -Dm0644 packaging/linux/app-io.github.ntrpydev.openfrag.service \
    %{buildroot}%{_userunitdir}/app-io.github.ntrpydev.openfrag.service
sed -i 's|ExecStart=%h/.local/bin/openfragd|ExecStart=%{_bindir}/openfragd|' \
    %{buildroot}%{_userunitdir}/app-io.github.ntrpydev.openfrag.service

%files
%license LICENSE
%license packaging/linux/THIRD_PARTY_NOTICES.md
%{_bindir}/openfragd
%{_bindir}/openfrag-launch
%{_datadir}/applications/io.github.ntrpydev.openfrag.desktop
%{_userunitdir}/app-io.github.ntrpydev.openfrag.service

%changelog
* Wed Jul 15 2026 NtrpyDev <ntrpydev@users.noreply.github.com> - 1.0.0-1
- Initial openfrag package
