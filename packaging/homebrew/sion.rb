# Homebrew formula for `sion`, built from source. There are no releases
# yet, so it builds the repository's head:
#
#   brew install --HEAD ./packaging/homebrew/sion.rb
#
# Once a release is tagged, a `url` and its `sha256` go here, and the
# formula can move to a tap.
class Sion < Formula
  desc "Siontology: a post-quantum neo-blockchain for agents"
  homepage "https://siontology.org"
  license any_of: ["Apache-2.0", "MIT"]
  head "https://github.com/ColdAI-org/siontology.git", branch: "master"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/sion-cli")
  end

  test do
    assert_match "sion #{version.to_s.sub(/^HEAD-.*/, "")}", shell_output("#{bin}/sion --version")
    assert_match "home", shell_output("#{bin}/sion doctor")
  end
end
