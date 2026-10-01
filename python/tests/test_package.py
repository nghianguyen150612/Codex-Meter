import codex_meter


def test_package_identity() -> None:
    assert codex_meter.PRODUCT_NAME == "Codex Meter"
    assert codex_meter.__version__ == "0.0.0"
