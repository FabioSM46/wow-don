# AUR package

`PKGBUILD` is the recipe of the `wow-don-bin` package for the AUR, which repackages the official `.deb`
of a release (D45).

For a new release:

1. Set `pkgver` and reset `pkgrel` to `1`.
2. Replace `sha256sums` with the SHA-256 of the new `.deb`.
3. Run `makepkg --printsrcinfo > .SRCINFO` and `makepkg -si` on Arch.
4. Commit `PKGBUILD` and `.SRCINFO` to the AUR repository.
