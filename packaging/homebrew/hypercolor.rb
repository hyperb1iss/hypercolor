# typed: false
# frozen_string_literal: true

# Homebrew formula for Hypercolor
# Auto-updated by CI — do not edit SHA256 sums manually.

class Hypercolor < Formula
  desc "Open-source RGB lighting orchestration engine"
  homepage "https://github.com/hyperb1iss/hypercolor"
  version "VERSION_PLACEHOLDER"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/hypercolor-#{version}-macos-arm64.tar.gz"
      sha256 "SHA256_MACOS_ARM64"
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
    %w[hypercolor hyper hypercolor-tray hypercolor-tui hypercolor-open].each do |b|
      bin.install "bin/#{b}" if File.exist?("bin/#{b}")
    end

    # Web UI + bundled effects
    (share/"hypercolor").install "share/hypercolor/ui" if File.directory?("share/hypercolor/ui")
    (share/"hypercolor").install "share/hypercolor/effects" if File.directory?("share/hypercolor/effects")

    # Shell completions
    bash_completion.install "share/bash-completion/completions/hyper" if File.exist?("share/bash-completion/completions/hyper")
    zsh_completion.install "share/zsh/site-functions/_hyper" if File.exist?("share/zsh/site-functions/_hyper")
    fish_completion.install "share/fish/vendor_completions.d/hyper.fish" if File.exist?("share/fish/vendor_completions.d/hyper.fish")
  end

  def caveats
    <<~EOS
      To start Hypercolor as a background service:
        brew services start hypercolor

      To open the web UI:
        hypercolor-open

      The daemon listens on http://127.0.0.1:9420 by default.
    EOS
  end

  service do
    run [opt_bin/"hypercolor", "--ui-dir", share/"hypercolor/ui"]
    keep_alive true
    log_path var/"log/hypercolor/hypercolor.log"
    error_log_path var/"log/hypercolor/hypercolor.log"
    environment_variables HYPERCOLOR_LOG: "info"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hypercolor --version")
    assert_match version.to_s, shell_output("#{bin}/hyper --version")
  end
end
