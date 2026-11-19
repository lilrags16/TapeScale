# Third-party runtime notices

TapeScale's original Rust and browser source is licensed under Apache-2.0 as stated in [`LICENSE`](LICENSE). The runtime image also distributes third-party software under its own licenses. That software is not relicensed under TapeScale's Apache-2.0 license.

## DAR and libthreadar

The runtime image includes these separately executed archive tools, built without source changes from their upstream release archives:

- DAR `2.8.6`, licensed under GPL-2.0-or-later
- libthreadar `1.6.1`, licensed under LGPL-3.0-or-later

The build enables DAR's multithreading, gzip, LZ4, Zstandard, extended-attribute, sparse-file, hard-link, and large-file support. Optional encryption, remote transport, delta, and additional compression libraries are not linked into this build. TapeScale invokes DAR as a separate process and does not link DAR or libthreadar code into the TapeScale Rust binaries.

The exact, unmodified corresponding source archives are included in the image:

- `/usr/share/doc/tapescale/sources/dar-2.8.6.tar.gz` (SHA-256 `d56caeb6c86f751cb454d8fef45f3daecc508f208290d9f51e8c75bba5dc46c5`)
- `/usr/share/doc/tapescale/sources/libthreadar-1.6.1.tar.gz` (SHA-256 `467709320c54215ac8829ef48e856d890caa4f6a7831633d3992e0b5359f38d8`)

License texts are installed at `/usr/share/doc/dar/COPYING` and `/usr/share/doc/libthreadar/`. Upstream project and release information is available from the [DAR project](https://dar.sourceforge.io/) and [libthreadar project](https://sourceforge.net/projects/libthreadar/).

## Other Debian packages

Copyright and license texts for the remaining runtime packages are retained under `/usr/share/doc/<package>/copyright` inside the image. The final published image digest and software bill of materials must be retained with each qualification report.
