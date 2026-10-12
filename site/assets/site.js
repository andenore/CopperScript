"use strict";

// The links still open the native illustrations when JavaScript is unavailable.
const views = {
  top: { image: "assets/routing-top.svg", alt: "Native KiCad F.Cu top copper and silkscreen of the recorded tracker review candidate", caption: "F.Cu · top copper and silkscreen. All views show the same recorded review candidate." },
  inner: { image: "assets/routing-inner.svg", alt: "Native KiCad In2.Cu regional modem supply and signal routing, viewed from above", caption: "In2.Cu · regional 3.8 V distribution and signal routing. Viewed from above without mirroring; return-path qualification remains open." },
  bottom: { image: "assets/routing-bottom.svg", alt: "Native KiCad B.Cu bottom copper routing, viewed from above without mirroring", caption: "B.Cu · bottom copper, viewed from above without mirroring. A single layer does not show every connection." },
  placement: { image: "assets/placement.svg", alt: "The same tracker component poses, pads and courtyards with copper hidden", caption: "Placement · the same recorded candidate poses with copper hidden. This is not a fresh unconstrained placement run." }
};

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
