class Herald < Formula
  desc "Local-first communication agent"
  homepage "https://github.com/haroon0x/herald"
  version "1.0.0"
  license "AGPL-3.0-or-later"

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/haroon0x/herald/releases/download/v1.0.0/herald-v1.0.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256_FOR_X86_64_LINUX"
    elsif Hardware::CPU.arm?
      url "https://github.com/haroon0x/herald/releases/download/v1.0.0/herald-v1.0.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256_FOR_AARCH64_LINUX"
    end
  end

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/haroon0x/herald/releases/download/v1.0.0/herald-v1.0.0-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256_FOR_X86_64_DARWIN"
    elsif Hardware::CPU.arm?
      url "https://github.com/haroon0x/herald/releases/download/v1.0.0/herald-v1.0.0-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256_FOR_AARCH64_DARWIN"
    end
  end

  def install
    bin.install "herald"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/herald --version")
  end
end
