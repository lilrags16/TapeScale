# Third-party runtime notices

TapeScale's original Rust and browser source is licensed under Apache-2.0 as stated in [`LICENSE`](LICENSE). The test runtime image also distributes unmodified third-party Debian packages under their own licenses. Those packages are not relicensed under TapeScale's Apache-2.0 license.

## Bacula Community

The G0 laboratory image installs these Debian bookworm-backports packages from Debian snapshot `20250730T000000Z`:

- `bacula-sd=15.0.3-3~bpo12+1`
- `bacula-bscan=15.0.3-3~bpo12+1`
- `bacula-common-pgsql=15.0.3-3~bpo12+1`

They provide the separately executed `/usr/sbin/btape` and future clean-catalog recovery utilities. Bacula Community is distributed under AGPLv3 with the additional terms and notices recorded by its authors and Debian maintainers. The installed authoritative notice is `/usr/share/doc/bacula-common/copyright` inside the image.

Corresponding source and Debian packaging for the exact build are available from the [Debian snapshot package record](https://snapshot.debian.org/package/bacula/15.0.3-3~bpo12%2B1/). The upstream release is available from the [Bacula Community source download center](https://www.bacula.org/source-download-center/).

TapeScale invokes Bacula as a separate process and does not modify or link Bacula code into the TapeScale binaries. Bacula names and trademarks belong to their respective owners; inclusion does not imply endorsement.

## Other Debian packages

Copyright and license texts for the remaining runtime packages are retained under `/usr/share/doc/<package>/copyright` inside the image. The final published image digest and software bill of materials must be retained with each qualification report.
