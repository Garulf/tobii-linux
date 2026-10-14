import unittest

from tobii_linux import eye_positioning as ep
from tobii_linux import gaze

# Synthetic guide samples built from the rules of Tobii's
# EyesPositioningParametersCalculator and EyesPositioningViewModel, NOT device traffic.
FRAME_US = 30_000


def samples(count, left=(0.55, 0.5, 0.5), right=(0.45, 0.5, 0.5), start_us=0):
    return [ep.GuideSample(start_us + i * FRAME_US, left, right) for i in range(count)]


def feed(guide, items):
    for item in items:
        guide.update(item)
    return guide


class CalculatorTests(unittest.TestCase):
    def test_eye_size_follows_the_distance_bands(self):
        self.assertEqual([ep.eye_size(d) for d in (0.1, 0.3, 0.5, 0.7, 0.9)], [100, 75, 50, 42, 25])

    def test_picture_is_mirrored(self):
        calculator = ep.EyesPositioning()
        calculator.update(ep.GuideSample(0, (0.7, 0.4, 0.5), None))
        self.assertAlmostEqual(calculator.left[0], 0.3)
        self.assertEqual(calculator.left[1], 0.4)

    def test_status_order_distance_before_sides(self):
        calculator = ep.EyesPositioning()
        self.assertEqual(calculator.update(ep.GuideSample(0, (0.95, 0.5, 0.9), None)), ep.Status.MOVE_CLOSER)
        calculator = ep.EyesPositioning()
        self.assertEqual(calculator.update(ep.GuideSample(0, (0.95, 0.5, 0.5), None)), ep.Status.MOVE_RIGHT)
        calculator = ep.EyesPositioning()
        self.assertEqual(calculator.update(ep.GuideSample(0, (0.5, 0.05, 0.5), None)), ep.Status.MOVE_DOWN)
        self.assertEqual(calculator.update(ep.GuideSample(1, None, None)), ep.Status.MOVE_DOWN)

    def test_a_missing_eye_is_extrapolated_from_the_last_two(self):
        guess = ep.extrapolated([(0.2, 0.5), (0.3, 0.5), None])
        self.assertAlmostEqual(guess[0], 0.4)
        self.assertAlmostEqual(guess[1], 0.5)
        self.assertEqual(ep.extrapolated([None, None]), None)
        self.assertEqual(ep.extrapolated([(0.9, 0.5), (1.0, 0.5), None]), (1.0, 0.5))

    def test_eye_colour_and_fade(self):
        self.assertEqual([ep.eye_grey(s) for s in (0, 25, 50, 75, 100)], [50, 50, 255, 152, 50])
        self.assertEqual(ep.eye_opacity((0.5, 0.5)), 1.0)
        self.assertAlmostEqual(ep.eye_opacity((0.5, 0.1)), 0.5)
        self.assertEqual(ep.eye_opacity((0.05, 0.5)), 0.0)

    def test_guide_sample_uses_the_validity_columns(self):
        g = gaze.DeviceGaze(timestamp=5, left_position_guide=(0.4, 0.5, 0.5), left_position_guide_validity=1,
                            right_position_guide=(0.6, 0.5, 0.5), right_position_guide_validity=0)
        self.assertEqual(ep.guide_sample(g), ep.GuideSample(5, (0.4, 0.5, 0.5), None))


class GuideTests(unittest.TestCase):
    def test_a_problem_shows_at_once_until_the_position_was_good(self):
        guide = feed(ep.PositioningGuide(), samples(1, (0.5, 0.5, 0.9), (0.4, 0.5, 0.9)))
        self.assertEqual(guide.hint, 'Move closer')

    def test_after_a_good_position_a_problem_needs_eleven_samples(self):
        guide = feed(ep.PositioningGuide(), samples(5))
        self.assertTrue(guide.good)
        too_close = samples(13, (0.5, 0.5, 0.1), (0.4, 0.5, 0.1), start_us=1_000_000)
        feed(guide, too_close[:12])
        self.assertEqual(guide.hint, '')
        self.assertEqual(guide.calculator.status, ep.Status.LEAN_BACK)
        feed(guide, too_close[12:])
        self.assertEqual(guide.hint, 'Lean back')

    def test_lost_eyes_ask_are_you_there_then_to_adjust(self):
        guide = feed(ep.PositioningGuide(), samples(1, None, None))
        self.assertEqual(guide.hint, '')
        feed(guide, samples(80, None, None, start_us=FRAME_US))
        self.assertEqual(guide.hint, 'Are you there?')
        feed(guide, samples(110, None, None, start_us=81 * FRAME_US))
        self.assertEqual(guide.hint, 'Adjust your seating position')

    def test_advances_after_six_seconds_with_three_seconds_of_presence(self):
        guide = feed(ep.PositioningGuide(), samples(1, None, None))
        feed(guide, samples(100, None, None, start_us=FRAME_US))
        good = samples(120, start_us=101 * FRAME_US)
        feed(guide, good[:100])
        self.assertFalse(guide.ready_to_advance(good[99].timestamp_us))
        feed(guide, good[100:])
        self.assertTrue(guide.ready_to_advance(good[-1].timestamp_us))

    def test_short_absence_keeps_presence(self):
        guide = feed(ep.PositioningGuide(), samples(150))
        feed(guide, samples(10, None, None, start_us=150 * FRAME_US))
        self.assertGreater(guide.presence_ms(160 * FRAME_US), 3000)
        feed(guide, samples(20, None, None, start_us=160 * FRAME_US))
        self.assertEqual(guide.presence_ms(180 * FRAME_US), 0.0)


if __name__ == '__main__':
    unittest.main()
