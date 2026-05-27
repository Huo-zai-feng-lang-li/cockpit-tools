cask "cockpit-tools" do
  version "0.20.56"
  sha256 "531ebd0b6d9cc907821d1cdd0a3ecfa28e991a5afcd017ffae42cb99399e416f"

  url "https://github.com/Huo-zai-feng-lang-li/cockpit-tools/releases/download/v#{version}/Cockpit.Tools_#{version}_x64.dmg",
      verified: "github.com/Huo-zai-feng-lang-li/cockpit-tools/"
  name "Cockpit Tools"
  desc "Account manager for AI IDEs (Antigravity and Codex)"
  homepage "https://github.com/Huo-zai-feng-lang-li/cockpit-tools"

  auto_updates true

  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-cr", "#{appdir}/Cockpit Tools.app"],
                   sudo: true
  end

  app "Cockpit Tools.app"

  zap trash: [
    "~/Library/Application Support/com.jlcodes.cockpit-tools",
    "~/Library/Caches/com.jlcodes.cockpit-tools",
    "~/Library/Preferences/com.jlcodes.cockpit-tools.plist",
    "~/Library/Saved Application State/com.jlcodes.cockpit-tools.savedState",
  ]

  caveats <<~EOS
    The app is automatically quarantined by macOS. A postflight hook has been added to remove this quarantine.
    If you still encounter the "App is damaged" error, please run:
      sudo xattr -rd com.apple.quarantine "/Applications/Cockpit Tools.app"
  EOS
end
