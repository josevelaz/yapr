---
title: Move to liatrio-labs
description: Steps to transfer the Yapr repository from josevelaz to the liatrio-labs GitHub organization.
---

# Move to liatrio-labs

The repository files already point to `liatrio-labs/yapr`. These steps move the repository on
GitHub so that those links work. Only the current owner (`josevelaz`) can start the transfer, and
the `liatrio-labs` organization must let that person create repositories in it.

## 1. Before the transfer

1. Make sure that `liatrio-labs` does not already have a repository named `yapr`.
2. Make sure that you can create repositories in `liatrio-labs`. If you cannot, ask a
   `liatrio-labs` owner to give you that permission. Another option is for the owner to approve
   the transfer request that GitHub sends to them.
3. Agree with the organization owners on the license in `LICENSE` and on the contact email in
   `CODE_OF_CONDUCT.md` (the `[CONTACT EMAIL]` placeholder).
4. Make sure that no release workflow run is in progress.

## 2. Transfer the repository

1. Open <https://github.com/josevelaz/yapr/settings>.
2. In **General**, go to **Danger Zone** and choose **Transfer ownership**.
3. Select **Specify an organization or username** and type `liatrio-labs`.
4. Type `josevelaz/yapr` to confirm, then choose **I understand, transfer this repository**.
5. If an organization owner must approve the transfer, ask them to accept it. The transfer is
   complete when <https://github.com/liatrio-labs/yapr> opens.

## 3. Check the settings in the organization

Organization policies can override repository settings. In
<https://github.com/liatrio-labs/yapr/settings>, check each item that
[docs/releases.md](releases.md#one-time-repository-setup) needs:

1. **Environments**: the `release` environment exists, with the `main` branch policy and the
   `RELEASE_SIGNING_P12_BASE64` and `RELEASE_SIGNING_P12_PASSWORD` secrets. If the secrets are
   missing, run `scripts/make-release-cert.sh` again. It reuses the certificate in
   `~/.config/yapr-release`, so users keep their macOS permissions.
2. **Actions → General**: Actions are allowed, and **Allow GitHub Actions to create and approve
   pull requests** is on. If the option is not available, ask an organization owner to allow it
   for the organization.
3. **General**: auto-merge and squash merge are on, and Immutable Releases is on.
4. **Rules**: the branch rules still match `docs/releases.md`.
5. **Security** settings: turn on **Private vulnerability reporting**. `SECURITY.md` tells
   people to use it.
6. **Collaborators and teams**: give the correct people and teams access.

Then run the release workflow once as a dry run, with the next version, to make sure that it
works:

```sh
gh workflow run Release --repo liatrio-labs/yapr --ref main \
  -f operation=cut -f version=X.Y.Z -f dry_run=true
```

## 4. Update your local clone

In each clone of the repository, point `origin` to the new location:

```sh
git remote set-url origin git@github.com:liatrio-labs/yapr.git
# or: git remote set-url origin https://github.com/liatrio-labs/yapr.git
git remote -v
git fetch origin
```

The `git remote -v` output shows `liatrio-labs/yapr`, and `git fetch` completes with no error.

## 5. Move the Homebrew tap

The tap is this repository: the release workflow updates `Casks/yapr.rb` on `main`, and the cask
downloads from `liatrio-labs/yapr` releases. You do not need to create another repository.

New users use the commands in the [README](../README.md#download).

Users who installed from the old tap keep getting updates, because GitHub redirects the old git
URL. Their tap still has the old name. To change to the new name, tell them to run:

```sh
brew uninstall --cask yapr
brew untap josevelaz/yapr
brew tap liatrio-labs/yapr https://github.com/liatrio-labs/yapr
brew install --cask liatrio-labs/yapr/yapr
```

`brew uninstall` without `--zap` keeps the settings. The release certificate does not change, so
macOS keeps the microphone, Accessibility, and Keychain grants.

## 6. After the move

1. GitHub redirects the old URLs (web pages, `git clone`, `git fetch`, the API, and release
   downloads) to `liatrio-labs/yapr`. Do not create a new repository named `josevelaz/yapr`,
   because that stops the redirects.
2. The repository files already use the new URLs. Update links in other places too, such as
   profiles, posts, and chat messages, so that they do not depend on the redirect.
3. When the brew tap and the Releases link in the README work, remove the note at the top of
   `README.md` in a small pull request.
