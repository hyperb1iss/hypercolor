# frozen_string_literal: true

# Homebrew cask for the Hypercolor desktop app.
# Auto-updated by CI — do not edit SHA256 sums manually.

cask "hypercolor-app" do
  arch arm: "arm64", intel: "x86_64"

  version "VERSION_PLACEHOLDER"
  sha256 arm:   "SHA256_MACOS_APP_ARM64",
         intel: "SHA256_MACOS_APP_X86_64"

  url "https://github.com/hyperb1iss/hypercolor/releases/download/v#{version}/Hypercolor-#{version}-#{arch}.dmg",
      verified: "github.com/hyperb1iss/hypercolor/"
  name "Hypercolor"
  desc "Open-source RGB lighting orchestration"
  homepage "https://github.com/hyperb1iss/hypercolor"

  app "Hypercolor.app"

  zap trash: [
    "~/Library/Application Support/hypercolor",
    "~/Library/Caches/hypercolor",
    "~/Library/Logs/Hypercolor",
    "~/Library/LaunchAgents/tech.hyperbliss.hypercolor.app.plist",
  ]
end
