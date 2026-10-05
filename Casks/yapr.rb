cask "yapr" do
  version "0.1.0"
  sha256 "fcac1fe8ff7d933261a5fffaffe27a8eb84818cb41e9274c70119b28e933a99d"

  url "https://github.com/josevelaz/yapr/releases/download/#{version}/Yapr-#{version}.zip"
  name "Yapr"
  desc "Menu bar dictation through Vercel AI Gateway"
  homepage "https://github.com/josevelaz/yapr"

  depends_on arch: :arm64
  depends_on macos: :sonoma

  app "Yapr.app"

  zap trash: "~/Library/Application Support/Yapr"

  caveats <<~EOS
    Yapr is not notarized by Apple, so macOS blocks its first launch.
    Open it once, then choose System Settings → Privacy & Security → Open Anyway.
    Or run:
      xattr -dr com.apple.quarantine #{appdir}/Yapr.app
  EOS
end
