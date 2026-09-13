# Hypercolor from the official Linux release tarball.
#
# This mirrors packaging/aur/PKGBUILD: the per-architecture tarball that CI
# builds on Ubuntu 24.04 is unpacked into the store and its ELF binaries are
# re-linked against nixpkgs libraries with autoPatchelf. A from-source
# derivation is deliberately out of scope for now: the daemon's default
# feature set builds Servo and SpiderMonkey, which is hours of compile and a
# build script that expects network access.
#
# `nix/release.json` pins the version and per-architecture checksums; the
# release pipeline refreshes it on every tagged release.
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  addDriverRunpath,
  makeWrapper,
  # Daemon: linked
  alsa-lib,
  fontconfig,
  freetype,
  pipewire,
  libpulseaudio,
  udev,
  zlib,
  # Daemon: dlopen'd by Servo, wgpu, and winit
  libGL,
  libglvnd,
  vulkan-loader,
  wayland,
  libxkbcommon,
  libx11,
  libxcb,
  libxcursor,
  libxi,
  libxrandr,
  # Desktop app shell (Tauri)
  gtk3,
  webkitgtk_4_1,
  libsoup_3,
  cairo,
  gdk-pixbuf,
  glib,
  dbus,
  libayatana-appindicator,
  xdotool,
  # hypercolor-open runtime
  curl,
  xdg-utils,
  release ? lib.importJSON ./release.json,
}:
let
  inherit (release) version;
  platform =
    {
      x86_64-linux = "linux-amd64";
      aarch64-linux = "linux-arm64";
    }
    .${stdenv.hostPlatform.system}
      or (throw "hypercolor: no release tarball for ${stdenv.hostPlatform.system}");
  sha256 = release.sha256.${stdenv.hostPlatform.system};
in
stdenv.mkDerivation {
  pname = "hypercolor";
  inherit version;

  src = fetchurl {
    url = "https://github.com/hyperb1iss/hypercolor/releases/download/v${version}/hypercolor-${version}-${platform}.tar.gz";
    inherit sha256;
  };

  sourceRoot = "hypercolor-${version}-${platform}";

  nativeBuildInputs = [
    autoPatchelfHook
    addDriverRunpath
    makeWrapper
  ];

  buildInputs = [
    stdenv.cc.cc.lib
    alsa-lib
    fontconfig
    freetype
    pipewire
    libpulseaudio
    udev
    zlib
    gtk3
    webkitgtk_4_1
    libsoup_3
    cairo
    gdk-pixbuf
    glib
    dbus
  ];

  # Libraries the binaries open at runtime rather than link against. They
  # land on the RUNPATH so dlopen finds them without any environment setup.
  runtimeDependencies = [
    libGL
    libglvnd
    vulkan-loader
    wayland
    libxkbcommon
    libx11
    libxcb
    libxcursor
    libxi
    libxrandr
    libayatana-appindicator
    xdotool
  ];

  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall

    mkdir -p $out/bin $out/share $out/lib/udev/rules.d $out/lib/systemd/user

    install -Dm755 bin/hypercolor-daemon $out/bin/hypercolor-daemon
    install -Dm755 bin/hypercolor        $out/bin/hypercolor
    install -Dm755 bin/hypercolor-app    $out/bin/hypercolor-app
    install -Dm755 bin/hypercolor-tui    $out/bin/hypercolor-tui
    install -Dm755 bin/hypercolor-open   $out/bin/hypercolor-open

    # Web UI, bundled effects, docs, agent skills, desktop entry, icons,
    # and shell completions keep the tarball layout so the daemon's
    # <prefix>/share/hypercolor discovery keeps working unchanged.
    cp -R share/. $out/share/

    # The desktop entry is stamped with the install prefix at dist time.
    substituteInPlace $out/share/applications/hypercolor.desktop \
      --replace-fail "Exec=/usr/bin/hypercolor-open" "Exec=$out/bin/hypercolor-open"

    cp lib/udev/rules.d/*.rules $out/lib/udev/rules.d/

    # Ship a user unit with store paths so non-NixOS systemd users can link
    # it directly. The NixOS module defines its own unit from options.
    sed \
      -e "s|/usr/bin/hypercolor-daemon|$out/bin/hypercolor-daemon|" \
      -e "s|/usr/share/hypercolor/ui|$out/share/hypercolor/ui --effects-dir $out/share/hypercolor/effects/bundled|" \
      lib/systemd/user/hypercolor.service.system \
      > $out/lib/systemd/user/hypercolor.service

    install -Dm644 etc/modules-load.d/i2c-dev.conf $out/lib/modules-load.d/i2c-dev.conf
    install -Dm644 LICENSE $out/share/licenses/hypercolor/LICENSE
    install -Dm644 NOTICE  $out/share/licenses/hypercolor/NOTICE

    runHook postInstall
  '';

  postFixup = ''
    # hypercolor-open shells out to systemctl, curl, and xdg-open.
    wrapProgram $out/bin/hypercolor-open \
      --prefix PATH : ${
        lib.makeBinPath [
          curl
          xdg-utils
        ]
      }

    # GPU drivers on NixOS live under /run/opengl-driver, which is not on
    # any RUNPATH nixpkgs knows about at build time.
    addDriverRunpath $out/bin/hypercolor-daemon
  '';

  passthru.platform = platform;

  meta = {
    description = "Open-source RGB lighting orchestration engine";
    longDescription = ''
      Hypercolor drives USB, HID, SMBus, and network RGB hardware from one
      spatially aware render pipeline, with HTML and native effects, a web
      UI, a TUI, a CLI, and a desktop app shell.
    '';
    homepage = "https://hypercolor.lighting";
    changelog = "https://github.com/hyperb1iss/hypercolor/blob/v${version}/CHANGELOG.md";
    license = lib.licenses.asl20;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    mainProgram = "hypercolor";
  };
}
