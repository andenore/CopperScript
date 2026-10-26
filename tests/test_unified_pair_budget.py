from dataclasses import replace
import pytest
from pcbir.unified_router import UnifiedRouterOptions


@pytest.mark.parametrize("limit", (0,-1,True,2.5))
def test_invalid_pair_caps_are_rejected(limit):
    with pytest.raises(ValueError,match="critical pair state limit"):
        UnifiedRouterOptions(critical_pair_state_limit=limit)
