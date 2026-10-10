# Metadata corpus

The metadata crate defines the first-milestone support matrix in one place.
It recognizes JPEG, HEIC, DNG, Canon CR3, Nikon NEF, Sony ARW, MOV, and MP4
originals. It also recognizes XMP, AAE, DxO DOP, and RawTherapee PP3 sidecars.

`tests/fixtures/corpus.json` represents common Canon, Nikon, Sony, Apple, and
Google device and date-source combinations. The corpus verifies normalized
metadata precedence, relationship rules, explicit exclusions, and canonical
counter allocation without depending on filesystem traversal order.

The corpus contains normalized metadata records instead of copyrighted camera
samples. Parser-specific byte extraction remains a separate validation step
when the project selects a metadata library. Unknown formats remain on the
source and appear as unhandled content.
