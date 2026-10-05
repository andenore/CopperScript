COPPER_ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
PROJECT_ROOT := $(COPPER_ROOT)
EXAMPLE ?= valid
include $(COPPER_ROOT)/make/examples/$(EXAMPLE).mk
include $(COPPER_ROOT)/make/board.mk

# Negative fixtures deliberately do not belong in this success aggregate.
EXAMPLES := valid hierarchical cm4 full-vertical nrf52 round-led-ring \
 mechanical-outline mechanical-editor mechanical-anchors mechanical-curves mechanical-reference mechanical-profile device-model \
 resolved-footprint rc-filter inrush
# These examples contain complete footprint identities and support physical
# layout/editor smoke checks. The remaining examples are electrical or
# simulation fixtures and are checked through check/compile and route recipe
# expansion only.
PHYSICAL_EXAMPLES := cm4 full-vertical nrf52 round-led-ring \
 mechanical-outline mechanical-editor mechanical-anchors mechanical-curves mechanical-reference mechanical-profile \
 resolved-footprint
.PHONY: check-examples compile-examples
.PHONY: $(addprefix check-example-,$(EXAMPLES)) $(addprefix compile-example-,$(EXAMPLES))

check-examples: $(addprefix check-example-,$(EXAMPLES))

$(addprefix check-example-,$(EXAMPLES)): check-example-%:
	$(MAKE) EXAMPLE=$* check
	$(MAKE) EXAMPLE=$* compile
	$(MAKE) -n EXAMPLE=$* route

compile-examples: $(addprefix compile-example-,$(EXAMPLES))
$(addprefix compile-example-,$(EXAMPLES)): compile-example-%:
	$(MAKE) EXAMPLE=$* compile
