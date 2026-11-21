"""Tests of `measurement.figures.register`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement import config
from measurement import inputs
from measurement.figures import declaration
from measurement.figures import register
from measurement.staleness import outside_sections


class Register(unittest.TestCase):
    def test_ids_are_unique(self):
        ids = [f.id for f in register.ALL_FIGURES]
        self.assertEqual(len(ids), len(set(ids)))

    def test_every_figure_declares_what_invalidates_it(self):
        # A figure that cannot say what invalidates it is one nobody has
        # thought about.
        for fig in register.ALL_FIGURES:
            with self.subTest(figure=fig.id):
                self.assertTrue(fig.depends)

    def test_every_declared_path_exists(self):
        for fig in register.ALL_FIGURES:
            for dep in fig.depends:
                with self.subTest(figure=fig.id, dep=dep):
                    self.assertTrue((config.REPO / dep).exists(), dep)

    def test_every_named_input_is_defined(self):
        for fig in register.FIGURES:
            for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
                with self.subTest(figure=fig.id, input=name):
                    self.assertIn(name, inputs.INPUTS)

    def test_a_borrower_is_declared_after_what_it_borrows_from(self):
        order = [f.id for f in register.FIGURES]
        for fig in register.FIGURES:
            for dep in fig.requires:
                with self.subTest(figure=fig.id, requires=dep):
                    self.assertLess(order.index(dep), order.index(fig.id))

    def test_every_defined_input_is_consumed(self):
        """An input no figure takes is a control that goes stale in silence.

        `one_block` was exactly that: the doc quoted a one-block parse at
        "under 10 ms" and gave the command that regenerates the *input*, while
        no figure took the reading — so nothing re-took it when the code it
        controls for moved."""
        used = {
            n
            for f in register.SELECTABLE
            for n in (*f.cold_inputs, *f.warm_inputs, *f.nvme_inputs)
        }
        self.assertEqual(sorted(set(inputs.INPUTS) - used), [])

    def test_the_quadratic_carries_its_one_block_control(self):
        # The series says a cost rises with block count; only the control says
        # how much of the cost *is* block count.
        fig = register.FIGURES_BY_ID["per-block-quadratic"]
        self.assertIn("one_block", fig.warm_inputs)
        self.assertEqual(register._QUADRATIC_ROWS[0][0], "one_block")

    def test_every_generator_named_by_an_input_exists(self):
        for spec in inputs.INPUTS.values():
            with self.subTest(input=spec.name):
                self.assertTrue((config.SCRIPTS / spec.generator).exists())


class Untaken(unittest.TestCase):
    """An instrument that is built and whose figure has not been taken.

    It has to be selectable and runnable, and it must not be mistaken for a
    figure the doc is missing — those pull in opposite directions, which is why
    the two registers are separate.

    What an entry waits on is a **commit to name** as its sitting, since a
    figure published outside a stamped sweep declares one inside its own marker
    and a sitting taken from a tree that carries the instrument uncommitted has
    none. `xz-decode-scaling`, the two `parallel-*` figures, `release-profile`,
    `inflate-throughput`, `gzseek-backend` and `gzseek-spacing` each waited
    here for exactly that and left when the commit existed."""

    def test_an_untaken_instrument_is_not_a_figure_the_doc_must_carry(self):
        for fig in register.UNTAKEN:
            with self.subTest(figure=fig.id):
                self.assertNotIn(fig, register.ALL_FIGURES)

    def test_a_sweep_does_not_take_it(self):
        # `--all` is `FIGURES`; an untaken instrument needs asking for by name.
        for fig in register.UNTAKEN:
            with self.subTest(figure=fig.id):
                self.assertNotIn(fig.id, [f.id for f in register.FIGURES])

    def test_it_is_still_selectable_by_name(self):
        for fig in register.UNTAKEN:
            with self.subTest(figure=fig.id):
                got = [f.id for f in outside_sections.resolve_selection([fig.id])]
                self.assertEqual(got, [fig.id])

    def test_its_inputs_and_declared_paths_are_real(self):
        # The structural checks `Register` applies to a figure apply here too:
        # an instrument nobody can stage is not built.
        for fig in register.UNTAKEN:
            for name in (*fig.cold_inputs, *fig.warm_inputs, *fig.nvme_inputs):
                with self.subTest(figure=fig.id, input=name):
                    self.assertIn(name, inputs.INPUTS)
            for path in fig.depends:
                with self.subTest(figure=fig.id, path=path):
                    self.assertTrue((config.REPO / path).exists(), path)

    def test_no_id_collides_with_a_figure(self):
        ids = [f.id for f in register.ALL_FIGURES] + [f.id for f in register.UNTAKEN]
        self.assertEqual(len(ids), len(set(ids)))


class SharedSections(unittest.TestCase):
    def test_the_two_throughput_tables_share_one_section(self):
        # The INSERT path's per-byte CPU is then a division within one place,
        # which is the defect that allocated the warm table.
        cold = register.FIGURES_BY_ID["scan-throughput-cold"]
        warm = register.FIGURES_BY_ID["scan-throughput-warm"]
        self.assertEqual(cold.section, warm.section)

    def test_a_shared_section_labels_each_table(self):
        by_section: dict[str, list[declaration.Figure]] = {}
        for fig in register.FIGURES:
            by_section.setdefault(fig.section, []).append(fig)
        for section, figs in by_section.items():
            if len(figs) > 1:
                for fig in figs:
                    with self.subTest(section=section, figure=fig.id):
                        self.assertTrue(fig.table_label)


if __name__ == "__main__":
    unittest.main()
