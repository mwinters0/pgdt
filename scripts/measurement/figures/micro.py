"""`nested-decode-micro` and `resumable-reader-micro`: the criterion benches, run in the image."""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence
from typing import TYPE_CHECKING

import release

from measurement import config
from measurement import formatting
from measurement import host
from measurement import parsing
from measurement.builds import image

if TYPE_CHECKING:
    from measurement.parsing import Estimate
    from measurement.timing import Session


# -- the nested decode micro ------------------------------------------------

# The literals the bench builds, reproduced so the byte counts in the table are
# computed rather than remembered: `int_array_literal(n)` is `{` plus n copies
# of a constant-width 11-byte int joined by commas plus `}`.
_RECORD_LITERAL = '(-1234567890,"lorem ipsum dolor sit amet")'


def array_literal_bytes(n: int) -> int:
    return 12 * n + 1

_MICRO_ROWS = (
    ("`integer[]`, 4 elements", array_literal_bytes(4), "array_4", "text_copy/array_4_len"),
    ("`integer[]`, 50 elements", array_literal_bytes(50), "array_50", "text_copy/array_50_len"),
    ("two-field composite", len(_RECORD_LITERAL), "record_2", "text_copy/record_2_len"),
)

VIEW_BATCH = 1024

#: `nested-decode-micro`'s program: criterion's `nested` group of
#: `benches/decoders.rs`.
NESTED_BENCH = release.Bench("pgdt-codec", "decoders", "nested")


def bench_in_image(
    session: Session, figure: str, bench: release.Bench, target_dir: Path | None = None
) -> Path:
    """`bench` run in the release image (`release.step_bench`), recorded as
    where `figure` ran, and the directory criterion wrote its estimates to.
    `target_dir` is a cargo target directory of its own (`image_target_dir`),
    for a bench stating `rustflags`; `None` is the release state's.

    **A bench runs in the release image**, not in `Config.image` as a built
    binary does: `cargo bench` builds and runs in one step, so the compiler
    that builds it and the glibc it runs under are both the release image's,
    itself built from the register image's digest. The image's tag is asked for
    its glibc as any place is (`Session.ran_in`). A dry run builds no image, so
    it names the tag the image's inputs give."""
    cfg = session.cfg
    if bench.rustflags and target_dir is None:
        raise ValueError(f"{figure}: a bench stating rustflags needs a target dir of its own")
    container = cfg.container_argv()
    if cfg.dry_run:
        tag = release.image_tag(release.image_inputs())
    else:
        tag = release.ensure_image(container)
    session.ran_in(figure, tag)
    if not cfg.dry_run:
        release.prepare_state(target=target_dir)
        host.run(
            release.run_in_image(container, tag, bench.step(), target=target_dir, reserve=0),
            cwd=config.REPO,
        )
    return bench.criterion_root(target_dir if target_dir is not None else release.STATE / "target")


def run_nested_decode_micro(session: Session) -> str:
    cfg = session.cfg
    root = bench_in_image(session, "nested-decode-micro", NESTED_BENCH) / "nested"
    if cfg.dry_run:
        return "(dry run: criterion not executed)"
    view_ns = parsing.criterion_median_ns(root, "nested/text_view_x1024") / VIEW_BATCH
    rows = []
    for label, nbytes, bench, control in _MICRO_ROWS:
        decode = parsing.criterion_median_ns(root, f"nested/{bench}/decode")
        render = parsing.criterion_median_ns(root, f"nested/{bench}/render")
        copy = parsing.criterion_median_ns(root, f"nested/{control}")
        rows.append(
            [
                label,
                str(nbytes),
                formatting._fmt_ns(decode),
                formatting._fmt_ns(render),
                f"**{(decode + render) / copy:.1f}×**",
                f"**{(decode + render) / view_ns:.0f}×**",
            ]
        )
    table = formatting.md_table(
        ["Literal", "Bytes", "`decode`", "`render`", "÷ copy", "÷ view"], rows
    )
    controls = "\n".join(
        f"- copy control at {nbytes} bytes: {formatting._fmt_ns(parsing.criterion_median_ns(root, f'nested/{control}'))}"
        for _, nbytes, _, control in _MICRO_ROWS
    )
    slope_d = (
        parsing.criterion_median_ns(root, "nested/array_50/decode")
        - parsing.criterion_median_ns(root, "nested/array_4/decode")
    ) / 46
    slope_r = (
        parsing.criterion_median_ns(root, "nested/array_50/render")
        - parsing.criterion_median_ns(root, "nested/array_4/render")
    ) / 46
    notes = (
        f"\nThe view control is `text_view_x1024`'s median ÷ {VIEW_BATCH} = **{view_ns:.2f} ns**, "
        "a floor on the borrowed arm rather than the borrowed arm itself, so the `÷ view` column "
        "bounds the real ratio from above.\n"
        f"Per-element slope between the two array lengths: **{slope_d:.0f} ns** decoding and "
        f"**{slope_r:.0f} ns** rendering.\n\nCriterion medians (ns):\n" + controls + "\n"
    )
    return table + notes

# -- the resumable reader --------------------------------------------------

#: `resumable-reader-micro`'s program: `benches/resumable/`'s one group, one
#: reader in three forms over each literal and feed.
RESUMABLE_BENCH = release.Bench("pgdt-codec", "resumable", "resumable")

#: The figure's layout leg: the same bench built with every function aligned
#: to 64 bytes, so a gap between two forms that moves with where the linker
#: put them is seen moving (`docs/design/measurements.md`, "Two builds of one
#: source can differ by layout"). Its own target dir, `RESUMABLE_ALIGNED_DIR`
#: under `image_target_dir`, since the flags fingerprint every crate.
RESUMABLE_ALIGNED_BENCH = dataclasses.replace(
    RESUMABLE_BENCH, rustflags="-C llvm-args=-align-all-functions=6"
)

RESUMABLE_ALIGNED_DIR = "resumable-aligned"

#: The bench's literals, in its order: `nested-decode-micro`'s three and a
#: scalar, each with its label and its byte count, computed as that figure's
#: are rather than remembered.
RESUMABLE_LITERALS = (
    ("int8", "`bigint`", len("-1234567890")),
    ("array_4", "`integer[]`, 4 elements", array_literal_bytes(4)),
    ("array_50", "`integer[]`, 50 elements", array_literal_bytes(50)),
    ("record_2", "two-field composite", len(_RECORD_LITERAL)),
)

#: The feeds: the field as one run, and as a run a byte.
RESUMABLE_FEEDS = (("whole", "one run"), ("bytes", "a run a byte"))

#: The forms, as the bench's ids name them; `pull_again` is `pull` timed again
#: last, after every other form.
RESUMABLE_FORMS = ("pull", "async", "push", "pull_again")


@dataclass(frozen=True)
class ResumableForms:
    """One literal over one feed in one build, every form's estimate."""

    pull: Estimate
    pull_again: Estimate
    async_: Estimate
    push: Estimate

    @property
    def pull_ns(self) -> float:
        """Today's pull, read as the mean of its two timings."""
        return (self.pull.median + self.pull_again.median) / 2

    @property
    def async_gap(self) -> float:
        """What the `async` walk costs over today's pull."""
        return self.async_.median - self.pull_ns

    @property
    def push_gap(self) -> float:
        """What the push machine costs over the `async` walk."""
        return self.push.median - self.async_.median


@dataclass(frozen=True)
class ResumableRow:
    """One literal over one feed: the shipped build's forms, which every gap
    is read off, and the layout leg's, which only widens a noise."""

    literal: str
    feed: str
    nbytes: int
    shipped: ResumableForms
    aligned: ResumableForms

    @property
    def drift_ns(self) -> float:
        """The sitting's own drift: what separates the pull's two timings."""
        return abs(self.shipped.pull.median - self.shipped.pull_again.median)

    @property
    def layout_async_ns(self) -> float:
        """How far `async − pull` moves between the shipped build and the
        layout leg."""
        return abs(self.aligned.async_gap - self.shipped.async_gap)

    @property
    def layout_push_ns(self) -> float:
        """How far `push − async` moves between the two builds."""
        return abs(self.aligned.push_gap - self.shipped.push_gap)

    @property
    def noise_ns(self) -> float:
        """**What the `async` walk is read against today's pull by**: the
        larger of the sitting's drift and the two medians' 95% intervals'
        half-widths summed, the wider of the pull's two, widened by the layout
        term. The first two were stated before any reading was taken, the
        layout term with the maintainer before any either
        (`docs/status/history/2026-11-03.md`)."""
        f = self.shipped
        pull_half = max(f.pull.half_width, f.pull_again.half_width)
        return max(self.drift_ns, pull_half + f.async_.half_width) + self.layout_async_ns

    @property
    def push_noise_ns(self) -> float:
        """**What the push machine is read against the `async` walk by**,
        formed as `noise_ns` is: the larger of the sitting's drift and the two
        medians' half-widths summed, widened by `push − async`'s layout term."""
        f = self.shipped
        return max(self.drift_ns, f.push.half_width + f.async_.half_width) + self.layout_push_ns

    @property
    def async_within(self) -> bool:
        return self.shipped.async_gap <= self.noise_ns

    @property
    def push_pays(self) -> bool:
        """Whether the push machine is cheaper than the `async` walk past
        their noise."""
        return -self.shipped.push_gap > self.push_noise_ns


def resumable_forms(root: Path, literal: str, feed: str) -> ResumableForms:
    est = {
        form: parsing.criterion_estimate(root, f"resumable/{literal}/{form}/{feed}")
        for form in RESUMABLE_FORMS
    }
    return ResumableForms(est["pull"], est["pull_again"], est["async"], est["push"])


def resumable_rows(root: Path, aligned_root: Path) -> list[ResumableRow]:
    """Every literal and feed's row, read out of each build's criterion
    estimates."""
    return [
        ResumableRow(
            literal, feed, nbytes,
            resumable_forms(root, literal, feed), resumable_forms(aligned_root, literal, feed),
        )  # fmt: skip
        for literal, _, nbytes in RESUMABLE_LITERALS
        for feed, _ in RESUMABLE_FEEDS
    ]


def resumable_verdict(rows: Sequence[ResumableRow]) -> str:
    """**The form the input walks resume in**: the `async` walk where it is
    within its noise of today's pull on every one-run row, the field as a
    reader meets it on nearly every line; past it on some, the push machine
    only if it is cheaper than the `async` walk past their noise on every one
    of those, and the `async` walk otherwise, neither form holding today's
    price. `pull` is the bar, never a candidate, resuming at no run. A row fed
    a byte a run prices a resumption and decides nothing. *Rejected:* those
    rows deciding too — a field reaches its reader split only where the
    carry's `FieldFold` splits it, once a chunk, so they price a boundary a
    field almost never crosses."""
    deciding = [row for row in rows if row.feed == "whole"]
    if not deciding:
        raise ValueError("no one-run row to decide by")
    beyond = [row for row in deciding if not row.async_within]
    if not beyond:
        return (
            "**The `async` walk is within its noise of today's pull on every one-run row**, "
            "so the input walks resume as `async` walks."
        )
    named = ", ".join(f"`{row.literal}`" for row in beyond)
    if all(row.push_pays for row in beyond):
        return (
            f"**The `async` walk is past its noise of today's pull on {named}, and the push "
            "machine cheaper than it past theirs on each**, so the input walks resume as "
            "hand-written push machines."
        )
    unpaid = ", ".join(f"`{row.literal}`" for row in beyond if not row.push_pays)
    return (
        f"**The `async` walk is past its noise of today's pull on {named}, and the push "
        f"machine not cheaper than it past theirs on {unpaid}**, so the input walks resume as "
        "`async` walks, neither form holding today's price."
    )


def render_resumable(rows: Sequence[ResumableRow]) -> str:
    labels = {literal: label for literal, label, _ in RESUMABLE_LITERALS}
    feeds = dict(RESUMABLE_FEEDS)
    criterion = (
        "Each row is one literal over one feed, every form's criterion median in the shipped "
        "build. **The criterion, stated before the readings**: a one-run row's `async` walk is "
        "within its noise of today's pull where it is slower than the pull's two timings' mean "
        "by no more than the larger of what separates those two timings and the medians' 95% "
        "intervals' half-widths summed, widened by how far that gap moves in the same bench "
        "built with every function aligned to 64 bytes. The `async` walk is taken if every "
        "one-run row is within; otherwise the push machine, if on every row that is not it "
        "is cheaper than the `async` walk by more than their noise, formed alike and widened "
        "by how far `push` − `async` moves between the builds; otherwise the `async` walk. A "
        "row fed a byte a run decides nothing: today's pull resumes at no run, and reads one "
        "chaining them.\n\n"
    )
    table = formatting.md_table(
        ["Literal", "Bytes", "Feed", "`pull`", "`pull` again", "`async`", "`push`",
         "`async` − `pull`", "Noise", "Within", "`push` − `async`", "Noise", "Pays"],
        [
            [
                labels[row.literal],
                str(row.nbytes),
                feeds[row.feed],
                formatting._fmt_ns(row.shipped.pull.median),
                formatting._fmt_ns(row.shipped.pull_again.median),
                formatting._fmt_ns(row.shipped.async_.median),
                formatting._fmt_ns(row.shipped.push.median),
                f"{row.shipped.async_gap:+.0f} ns",
                formatting._fmt_ns(row.noise_ns),
                ("yes" if row.async_within else "**no**") if row.feed == "whole" else "—",
                f"{row.shipped.push_gap:+.0f} ns",
                formatting._fmt_ns(row.push_noise_ns),
                ("yes" if row.push_pays else "no") if row.feed == "whole" else "—",
            ]
            for row in rows
        ],
    )  # fmt: skip
    layout = "\n".join(
        f"- {labels[row.literal]}, {feeds[row.feed]}: `async` − `pull` "
        f"{row.aligned.async_gap:+.0f} ns, `push` − `async` {row.aligned.push_gap:+.0f} ns"
        for row in rows
    )
    whole = {row.literal: row.shipped for row in rows if row.feed == "whole"}
    resumption = "\n".join(
        f"- {labels[row.literal]}: `async` "
        f"{(row.shipped.async_.median - whole[row.literal].async_.median) / (row.nbytes - 1):.1f} ns, "
        f"`push` {(row.shipped.push.median - whole[row.literal].push.median) / (row.nbytes - 1):.1f} ns, "
        f"`pull` over the chained runs "
        f"{(row.shipped.pull_ns - whole[row.literal].pull_ns) / (row.nbytes - 1):.1f} ns"
        for row in rows
        if row.feed == "bytes"
    )
    return (
        criterion
        + table
        + "\n\nThe layout leg's gaps, every function aligned to 64 bytes, which the noise's "
        "layout term is each gap's distance from:\n"
        + layout
        + "\n\nWhat a run boundary costs, a byte-a-run row less its one-run row over the "
        "boundaries it adds:\n"
        + resumption
        + "\n\n"
        + resumable_verdict(rows)
        + "\n"
    )


def run_resumable_reader_micro(session: Session) -> str:
    root = bench_in_image(session, "resumable-reader-micro", RESUMABLE_BENCH) / "resumable"
    aligned = bench_in_image(
        session, "resumable-reader-micro", RESUMABLE_ALIGNED_BENCH,
        image.image_target_dir(session.cfg, RESUMABLE_ALIGNED_DIR),
    ) / "resumable"  # fmt: skip
    if session.cfg.dry_run:
        return "(dry run: criterion not executed)"
    return render_resumable(resumable_rows(root, aligned))
