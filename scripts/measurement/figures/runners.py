"""Every selectable figure's run function, by id: the top of the figures layer.

Bound here rather than on `Figure`: the session and the stager read a
declaration by id, so `figures.register` sits below them, and every run
function reads the session, so it sits above."""

from __future__ import annotations

from typing import Callable
from typing import TYPE_CHECKING

from measurement.figures import allocator
from measurement.figures import blocks
from measurement.figures import dynfilter
from measurement.figures import gathering
from measurement.figures import gzip_first_pass
from measurement.figures import gzip_resident
from measurement.figures import gzseek
from measurement.figures import inflate
from measurement.figures import long_line
from measurement.figures import micro
from measurement.figures import nested
from measurement.figures import parallel
from measurement.figures import prepass
from measurement.figures import purge_delay
from measurement.figures import query
from measurement.figures import release_profile
from measurement.figures import reserve
from measurement.figures import throughput
from measurement.figures import xz_decode

if TYPE_CHECKING:
    from measurement.timing import Session


#: Every figure a sweep takes, by id, to the function that takes it and returns
#: its section's body. Bound here rather than on `Figure`, because the register
#: is where the session and the stager read a figure's declaration, so it sits
#: below every function that takes one; a field naming the function would point
#: back up.
RUNS: dict[str, Callable[[Session], str]] = {
    "scan-throughput-cold": throughput.run_scan_throughput_cold,
    "scan-throughput-warm": throughput.run_scan_throughput_warm,
    "scan-throughput-nvme": throughput.run_scan_throughput_nvme,
    "chunk-size": throughput.run_chunk_size,
    "nested-end-to-end": nested.run_nested_end_to_end,
    "cross-file-floor": nested.run_cross_file_floor,
    "per-block-quadratic": blocks.run_per_block_quadratic,
    "peak-rss": blocks.run_peak_rss,
    "map-only": prepass.run_map_only,
    "preamble-prepass": prepass.run_preamble_prepass,
    "nested-decode-micro": micro.run_nested_decode_micro,
    "resumable-reader-micro": micro.run_resumable_reader_micro,
    "projection-widths": query.run_projection_widths,
    "predicate-terms": query.run_predicate_terms,
    "allocator": allocator.run_allocator,
    "release-profile": release_profile.run_release_profile,
    "xz-decode-scaling": xz_decode.run_xz_decode_scaling,
    "inflate-throughput": inflate.run_inflate_throughput,
    "gzseek-backend": gzseek.run_gzseek_backend,
    "gzseek-spacing": gzseek.run_gzseek_spacing,
    "parallel-scan-throughput": parallel.run_parallel_scan_throughput,
    "parallel-peak-rss": parallel.run_parallel_peak_rss,
    "rss-attribution": blocks.run_rss_attribution,
    "reserve": reserve.run_reserve,
    "statistics-gathering": gathering.run_statistics_gathering,
    "statistics-pruning": gathering.run_statistics_pruning,
    "dynamic-filter-join": dynfilter.run_dynamic_filter_join,
    "dynamic-filter-topk": dynfilter.run_dynamic_filter_topk,
    "gzip-resident": gzip_resident.run_gzip_resident,
    "gzip-first-pass": gzip_first_pass.run_gzip_first_pass,
    "long-line-resident": long_line.run_long_line_resident,
    "purge-delay": purge_delay.run_purge_delay,
    "purge-delay-resident": purge_delay.run_purge_delay_resident,
}
