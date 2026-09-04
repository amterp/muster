# Muster's Homebrew cask, kept here and copied to amterp/homebrew-tap.
#
# It lives in this repo because it has to change with the app - a new release
# moves `version` and `sha256`, and a new bundle layout moves the `binary` path -
# and a file that changes with the app should be reviewed alongside it. The tap
# is a different repository; the copy there is what `brew install` reads.
#
# `version` and `sha256` below name the current release. Both come from
# `./dev --notarize`, which prints the checksum of the artifact it just built;
# the release workflow also puts it in the release body, so it is still findable
# when somebody updates the tap a week later.
#
# One cask rather than a cask and a formula, because the app and the CLI are one
# artifact: `muster-cli` is a file inside the bundle, built from the same commit
# and signed with the same signature, and a formula would have to download the
# app a second time to get at it.

cask "muster" do
  version "0.6.0"
  sha256 "9eb79d42128b6189d431474e16a916f5079ff377db57edd1f7bc37587d36331f"

  url "https://github.com/amterp/muster/releases/download/v#{version}/Muster-#{version}-arm64.zip"
  name "Muster"
  desc "Native workspace for AI coding agents"
  homepage "https://github.com/amterp/muster"

  livecheck do
    url :url
    strategy :github_latest
  end

  # Two refusals brew can give better than macOS can. Sonoma is what the app's
  # LSMinimumSystemVersion says, and Apple Silicon is what the release is built
  # for - without the second, an Intel Mac downloads 60 MB and is told "you
  # can't open the application because it is not supported on this type of Mac".
  depends_on macos: :sonoma
  depends_on arch: :arm64

  # The app and the CLI arrive together, which is the whole reason this is a
  # cask. There is a second `muster` on a Muster user's PATH - the app keeps one
  # in ~/.muster/bin pointing at whichever build is running - and the two do not
  # compete over anything that matters: both find the window through
  # $MUSTER_SOCKET, so either drives the pane it is run in. Which of them a pane
  # finds first depends on the profile that shell loaded, and the only difference
  # it makes is which build answers.
  app "Muster.app"
  binary "#{appdir}/Muster.app/Contents/MacOS/muster-cli", target: "muster"

  # Quit first, because Muster owns a bridge process per pane and a copy of the
  # app deleted out from under a running one leaves those to be reaped. The
  # session daemon is meant to outlive the app and does: agents keep working,
  # and `zap` is what takes them down with everything else.
  #
  # `quit:` names a bundle id, so it closes any Muster carrying it - a build run
  # from a checkout included, not only the one brew installed. That is the right
  # behaviour for an uninstall and a sharp edge while testing this file: expect
  # a window you were working in to close.
  #
  # The link goes with an ordinary uninstall rather than waiting for `zap`,
  # because once the bundle is gone it points at nothing and the only thing that
  # would have repaired it is the app that just left. Deleted rather than
  # trashed: a dangling link on somebody's PATH is worse than no link, and a
  # dangling link in the Trash is just confusing.
  uninstall quit:   "dev.amterp.muster",
            delete: "~/.muster/bin/muster"

  zap trash: [
    "~/.muster",
    "~/Library/Preferences/dev.amterp.muster.plist",
    "~/Library/Saved Application State/dev.amterp.muster.savedState",
  ]

  caveats <<~EOS
    `muster` is now on your PATH and drives the running window:

      muster window
      muster pane new --down --run claude

    Muster keeps a second copy at ~/.muster/bin/muster pointing at whichever
    build is running. Either one drives the window a pane belongs to, so which
    your shell finds first only decides which build answers.

    Muster starts a session daemon that outlives the app on purpose, so quitting
    costs you nothing and your agents keep working. `brew uninstall --zap muster`
    is what stops them.

    The first launch may ask for permissions. They say Muster Sessions, which is
    the daemon that owns your panes, and the request came from a program running
    in one of them. Answering once covers every pane from then on, including
    after you quit and come back.
  EOS
end
