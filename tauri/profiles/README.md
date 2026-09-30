# Packaged DNG looks

## Merrill SPP 1.0

`merrill_spp_1_0.dcp` is the reviewed SPP-style look, targeted at Merrill images
and available for testing with other cameras. It is bundled but disabled by
default. Choosing it applies its look table and tone curve to all DNG exports,
without replacing camera calibration or raw samples.

The file preserves the reviewed profile byte for byte. SHA-256:

```
f28bad2412239e20a06bbbdb4d13ca467fb567b6a7598d47fe9bc1f7fc5fca4d
```

It does not affect editor previews, rendered JPEG/TIFF exports, or embedded
camera thumbnails. Look embedding does not require a preview renderer or
scripting runtime.
