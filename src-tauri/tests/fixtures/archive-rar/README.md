# RAR test fixtures

These files are used by offline tests. The ordinary archives contain inert
text, image, and nested-archive files. Two additional well-formed encrypted
archives exercise rejection of a RAR feature unsupported by the application;
no linked, multivolume, or malformed archive is included here.

| Local file | Source and provenance | Contents |
| --- | --- | --- |
| `rar4-legacy.rar` | `ssokolow/rar-test-files`, commit `16b785c2b1b504e99fc307676e5369a26d3ce060`, `build/testfile.rar3.rar`; copied byte-for-byte. Upstream labels this RAR3 because it was made with RAR 3.93. 7-Zip identifies it as legacy RAR, header version 29, compression method m5. | `testfile.txt`, 12 bytes (`Testing 123\n`). |
| `rar5-mixed.rar` | `ssokolow/rar-test-files`, same commit, `build/testfile.rar5.cbr`; copied byte-for-byte and renamed to `.rar` for the test. The source archive is ordinary RAR5 with one m5-compressed and one stored member. | `testfile.jpg` (220 bytes) and `testfile.png` (87 bytes). |
| `rar5-nested.rar` | `libarchive/libarchive`, commit `0fdf26e5135c4ce523dde176ea5f8d9f8e5ff704`, `libarchive/test/test_read_format_rar5_zip_in_rar.rar.uu`; decoded from uuencoding. | Directory `payload/`, `payload/inner.zip` (165 bytes), and `payload/real_after.txt` (21 bytes). |
| `rar4-encrypted.rar` | `libarchive/libarchive`, same commit, `libarchive/test/test_read_format_rar4_encrypted.rar.uu`; decoded from uuencoding. | Four ordinary text members, two encrypted; the complete archive must be rejected without partial extraction. |
| `rar5-encrypted.rar` | `libarchive/libarchive`, same commit, `libarchive/test/test_read_format_rar5_encrypted.rar.uu`; decoded from uuencoding. | Four ordinary text members, two encrypted; the complete archive must be rejected without partial extraction. |

The first two archives and their contents are distributed under CC0 1.0; see
[`CC0-1.0.txt`](CC0-1.0.txt). The other fixtures are distributed with the
libarchive source under BSD-2-Clause; see [`BSD-2-Clause.txt`](BSD-2-Clause.txt).

SHA256 digests of the checked-in archive bytes:

| File | SHA256 |
| --- | --- |
| `rar4-legacy.rar` | `dce342bc0c2852fcaa36a03da5e55abb7dd69c045bbd812faebebc1a3844f5a4` |
| `rar5-mixed.rar` | `e8b106048f18e6fb9a5f8ec6a95346e76906e7e4e9ca15ec97e4f926159cb398` |
| `rar5-nested.rar` | `4d0735b083c85575d19585cb2ae629267a046689b3ecacac6c1ee03902edef86` |
| `rar4-encrypted.rar` | `c30dc6d10e59aaab1c863afa469d188c644a22d67d6e50ac7ca887e520f5fc8b` |
| `rar5-encrypted.rar` | `72b69a1d5f5757511e683a9207166c60e91ce0846ae446b8b332b085f9e0ba48` |

All non-encrypted archives were checked locally with the project's bundled
7-Zip 26.06 (`7z t`) before being added. These checks are fixture provenance
checks; the Rust tests exercise the application archive APIs directly.
