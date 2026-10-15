"use strict";

// The links still open the native illustrations when JavaScript is unavailable.
const views = {
  all: { image: "assets/routing-all.svg", alt: "All six copper layers of CopperAssetTracker, with tracks, vias and gold package pads visible and copper fill areas hidden", caption: "All 6 copper layers · tracks, vias and gold package pads. Copper fill areas hidden; viewed from above. Same recorded review candidate." },
  top: { image: "assets/routing-top.svg", alt: "Native KiCad F.Cu top copper and silkscreen with package pads highlighted gold from the recorded 13 October 2026 route", caption: "F.Cu · top copper and silkscreen, with package pads highlighted gold. promo-reroute-20261013; unapproved routing review." },
  inner: { image: "assets/routing-inner.svg", alt: "Native KiCad In2.Cu regional modem supply and signal routing from the fresh 13 October route, viewed from above", caption: "In2.Cu · regional supply and signal routing from the same 13 October route. Viewed from above without mirroring; return-path qualification remains open." },
  bottom: { image: "assets/routing-bottom.svg", alt: "Native KiCad B.Cu bottom copper routing from the recorded 13 October route, with native bottom-side pads highlighted where present, viewed from above", caption: "B.Cu · bottom copper from the same 13 October route; surface pads highlighted where present. Viewed from above without mirroring. A single layer does not show every connection." },
  placement: { image: "assets/placement.svg", alt: "Native front package pads highlighted gold, fabrication outlines and courtyards for the same 97 footprints; tracks, vias and fills hidden", caption: "Placement · 97 footprints at their recorded routed poses. Gold package pads, native fabrication outlines and courtyards; tracks, vias and fills hidden." }
};

// Keep browser caches from mixing previous artwork with a new snapshot.
for (const view of Object.values(views)) view.image += "?v=20261015-pads";

const image = document.getElementById("board-view");
const caption = document.getElementById("board-caption");
const fullsize = document.getElementById("board-fullsize");
const links = document.querySelectorAll("[data-view]");
links.forEach(link => {
  link.addEventListener("click", event => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
    const view = views[link.dataset.view];
    if (!view || !image || !caption) return;
    event.preventDefault();
    image.src = view.image;
    image.alt = view.alt;
    caption.textContent = view.caption;
    if (fullsize) fullsize.href = view.image;
    links.forEach(item => item.removeAttribute("aria-current"));
    link.setAttribute("aria-current", "true");
  });
});

if (navigator.clipboard && window.isSecureContext) {
  document.querySelectorAll("[data-copy]").forEach(button => {
    button.hidden = false;
    button.addEventListener("click", async () => {
      const source = document.getElementById(button.dataset.copy);
      if (!source) return;
      try {
        await navigator.clipboard.writeText(source.textContent.trim());
        button.textContent = "Copied";
      } catch {
        button.textContent = "Select text to copy";
      }
      window.setTimeout(() => { button.textContent = "Copy commands"; }, 2500);
    });
  });
}
