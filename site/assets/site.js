"use strict";

// The links still open the native illustrations when JavaScript is unavailable.
const views = {
  top: { image: "assets/routing-top.svg", alt: "Native KiCad F.Cu top copper and silkscreen from the fresh 13 October 2026 source-matched route", caption: "F.Cu · top copper and silkscreen. promo-reroute-20261013; unapproved routing review." },
  inner: { image: "assets/routing-inner.svg", alt: "Native KiCad In2.Cu regional modem supply and signal routing from the fresh 13 October route, viewed from above", caption: "In2.Cu · regional supply and signal routing from the same 13 October route. Viewed from above without mirroring; return-path qualification remains open." },
  bottom: { image: "assets/routing-bottom.svg", alt: "Native KiCad B.Cu bottom copper routing from the fresh 13 October route, viewed from above without mirroring", caption: "B.Cu · bottom copper from the same 13 October route, viewed from above without mirroring. A single layer does not show every connection." },
  placement: { image: "assets/placement.svg", alt: "Native front fabrication and courtyard artwork for the same 97 footprints from the fresh route, with copper omitted", caption: "Placement · 97 footprints at their final routed poses. Native fabrication/courtyard artwork, with copper omitted; not an assembled-board photograph." }
};

// Keep browser caches from mixing previous artwork with a new snapshot.
for (const view of Object.values(views)) view.image += "?v=20261013";

const image = document.getElementById("board-view");
const caption = document.getElementById("board-caption");
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
