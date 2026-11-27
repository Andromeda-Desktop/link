# The Link window from a dist/window/build.sh stage.
# Build: rpmbuild -bb dist/window/andromeda-link-window.spec --define "stage $PWD/stage" --define "linkversion 0.2.17"
Name:           andromeda-link-window
Version:        %{linkversion}
Release:        1%{?dist}
Summary:        Andromeda Link window
License:        GPL-3.0-or-later
Requires:       andromeda-link = %{linkversion}
Provides:       andromeda-link-app
Obsoletes:      andromeda-link-app < 2027
%global debug_package %{nil}
%global __brp_strip %{nil}

%description
Pair the phone, mirror its screen, and use its files, texts and calls.

%install
cp -a %{stage}/. %{buildroot}/

%files
%{_bindir}/andromeda-link-app
%{_prefix}/lib/andromeda-link/andromeda-link-app
%{_prefix}/lib/andromeda-link/assets
%{_prefix}/lib/andromeda-link/translations
%{_datadir}/applications/dev.andromeda.Link.desktop
%{_datadir}/dbus-1/services/dev.andromeda.Link.service
%{_datadir}/icons/hicolor/*/apps/dev.andromeda.Link.png
