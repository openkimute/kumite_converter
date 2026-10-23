# Third-party notices

kumite_converter's own code is under either of [LICENSE-MIT](LICENSE-MIT) or [LICENSE-APACHE](LICENSE-APACHE), at your
option. It contains, or is derived from, the following third-party work. The Rust crates it depends on are under
their own licences (MIT, Apache-2.0, BSD, Zlib, MPL-2.0, ...).

## Pillow's resampling (`t2_converter/src/pil.rs`)

The Lanczos resize reimplements the algorithm of Pillow's `Resample.c` (8-bit fixed-point coefficients,
premultiplied alpha), so that pictures match those Pillow made.

    Copyright © 1997-2011 by Secret Labs AB
    Copyright © 1995-2011 by Fredrik Lundh and contributors
    Copyright © 2010 by Jeffrey 'Alex' Clark and contributors

    Permission to use, copy, modify and distribute this software and its documentation for any purpose and without
    fee is hereby granted, provided that the above copyright notice appears in all copies, and that both that
    copyright notice and this permission notice appear in supporting documentation, and that the name of Secret
    Labs AB or the author not be used in advertising or publicity pertaining to distribution of the software
    without specific, written prior permission.

    SECRET LABS AB AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE, INCLUDING ALL IMPLIED
    WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL SECRET LABS AB OR THE AUTHOR BE LIABLE FOR ANY
    SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR
    PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION
    WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

## No game data

The repository contains no data from Tekken 3, Tekken 2, Counter-Strike or Half-Life. Everything game-specific is
read by the converter from the user's own copies at run time.
