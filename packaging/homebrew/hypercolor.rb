# typed: false
# frozen_string_literal: true

# Homebrew formula for Hypercolor
# Updated manually after release artifacts pass signed acceptance.

class Hypercolor < Formula
  # Sequoia's symbolic version cannot distinguish 15.0 from the 15.2 floor.
  class MacosVersionRequirement < Requirement
    fatal true

    satisfy(build_env: false) do
      !OS.mac? || MacOS.version >= Version.new("15.2")
    end

    def message
      "Hypercolor requires macOS 15.2 or newer."
    end
  end

  desc "Open-source RGB lighting orchestration engine"
  homepage "https://github.com/hyperb1iss/hypercolor"
  version "VERSION_PLACEHOLDER"
  license "Apache-2.0"

  on_macos do
    depends_on macos: ">= :sequoia"
    depends_on MacosVersionRequirement

    if Hardware::CPU.arm?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-arm64.tar.gz"
      sha256 "SHA256_MACOS_ARM64"
    elsif Hardware::CPU.intel?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-amd64.tar.gz"
      sha256 "SHA256_MACOS_AMD64"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-linux-amd64.tar.gz"
      sha256 "SHA256_LINUX_AMD64"
    elsif Hardware::CPU.arm?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-linux-arm64.tar.gz"
      sha256 "SHA256_LINUX_ARM64"
    end
  end

  def install
    # Binaries
    %w[
      hypercolor-daemon
      hypercolor
      hypercolor-app
      hypercolor-tui
      hypercolor-open
    ].each do |b|
      bin.install "bin/#{b}" if File.exist?("bin/#{b}")
    end

    # Web UI + bundled effects
    (share/"hypercolor").install "share/hypercolor/ui" if File.directory?("share/hypercolor/ui")
    (share/"hypercolor").install "share/hypercolor/effects" if File.directory?("share/hypercolor/effects")

    # Shell completions
    bash_completion.install "share/bash-completion/completions/hypercolor" if File.exist?("share/bash-completion/completions/hypercolor")
    zsh_completion.install "share/zsh/site-functions/_hypercolor" if File.exist?("share/zsh/site-functions/_hypercolor")
    fish_completion.install "share/fish/vendor_completions.d/hypercolor.fish" if File.exist?("share/fish/vendor_completions.d/hypercolor.fish")
  end

  def caveats
    <<~EOS
      To start Hypercolor as a background service:
        brew services start hypercolor

      To open the web UI:
        hypercolor-open

      To launch the unified desktop app:
        hypercolor-app

      The daemon listens on http://127.0.0.1:9420 by default.
    EOS
  end

  service do
    run [opt_bin/"hypercolor-daemon", "--macos-owner", "homebrew", "--ui-dir", share/"hypercolor/ui"]
    keep_alive successful_exit: false
    log_path var/"log/hypercolor/hypercolor.log"
    error_log_path var/"log/hypercolor/hypercolor.log"
    environment_variables HYPERCOLOR_LOG: "info", HYPERCOLOR_MACOS_OWNER: "homebrew"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hypercolor --version")
    assert_match "Hypercolor lighting daemon", shell_output("#{bin}/hypercolor-daemon --help")
  end
end
