from backend.app.downloader.http_split import ENDGAME_SPLIT_MIN_BYTES, pick_endgame_split


def test_no_split_when_remaining_is_below_threshold():
    assert pick_endgame_split(
        live_parts={(0, 0): ENDGAME_SPLIT_MIN_BYTES - 2},
        partials={(0, 0): 0},
        completed=set(),
    ) is None


def test_splits_largest_live_part_including_non_primary_tail():
    picked = pick_endgame_split(
        live_parts={
            (0, 0): 1_000_000,
            (0, 2_000_000): 5_000_000,
            (1, 8_000_000): 8_200_000,
        },
        partials={(0, 0): 200_000, (0, 2_000_000): 0, (1, 8_000_000): 0},
        completed={1},
    )
    assert picked == (0, 2_000_000, 3_500_000, 5_000_000)


def test_completed_or_invalid_parts_are_ignored():
    assert pick_endgame_split(
        live_parts={(0, 10): 5, (1, 0): 8_000_000},
        partials={},
        completed={1},
        min_bytes=1024,
    ) is None
