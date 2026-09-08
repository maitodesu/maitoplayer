# Performance evidence

Release measurements use synthetic fixtures and
`docs/spikes/media-compatibility/measurement-template.md`. Record direct, remux,
audio-conversion, and video-conversion paths separately. A build passing unit
tests is not equivalent to meeting the release performance gate.

`release-measurements.md` defines the procedure. The process sampler records
working set, private memory, handles, threads, and cumulative CPU; browser timing,
dropped frames, GPU use, and range sizes must be captured separately.
