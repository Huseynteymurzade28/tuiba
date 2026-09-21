# AUR packaging

`PKGBUILD` builds the `tuiba` package from the tagged GitHub release
archive. To publish a new version:

```sh
cd packaging/aur
# bump pkgver, reset pkgrel to 1, then:
updpkgsums                          # fills sha256sums from the release tarball
makepkg --printsrcinfo > .SRCINFO
makepkg -f                          # build and test locally
```

Then copy `PKGBUILD` and `.SRCINFO` into a checkout of
`ssh://aur@aur.archlinux.org/tuiba.git` and push.
