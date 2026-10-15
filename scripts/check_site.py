"""Check the public static site without installing packages or building boards."""

from __future__ import annotations

import hashlib
import json
import re
import sys
import xml.etree.ElementTree as ET
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

from sync_branding import check_copies

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "site"
BASE_URL = "https://andenore.github.io/CopperScript/"
SNAPSHOT_ASSETS = (
    "routing-top.svg", "routing-inner.svg", "routing-bottom.svg", "routing-all.svg", "placement.svg", "rendering.json",
    "electrical-ir.excerpt.json", "physical-ir.excerpt.json",
)


class Page(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.ids: set[str] = set()
        self.duplicate_ids: list[str] = []
        self.references: list[str] = []
        self.images_without_alt = 0
        self.h1_count = 0
        self.canonical: str | None = None
        self.description: str | None = None

    def handle_starttag(self, tag: str, attributes: list[tuple[str, str | None]]) -> None:
        attrs = dict(attributes)
        element_id = attrs.get("id")
        if element_id:
            if element_id in self.ids:
                self.duplicate_ids.append(element_id)
            self.ids.add(element_id)
        if tag == "h1":
            self.h1_count += 1
        if tag == "img" and not attrs.get("alt"):
            self.images_without_alt += 1
        if tag == "link" and attrs.get("rel") == "canonical":
            self.canonical = attrs.get("href")
        if tag == "meta" and attrs.get("name") == "description":
            self.description = attrs.get("content")
        if tag == "meta" and attrs.get("property") == "og:image" and attrs.get("content"):
            self.references.append(attrs["content"])
        for key in ("href", "src"):
            if attrs.get(key):
                self.references.append(attrs[key])


def check_site(site: Path = SITE) -> list[str]:
    errors: list[str] = []
    pages: dict[Path, Page] = {}
    for path in site.rglob("*.html"):
        page = Page()
        page.feed(path.read_text(encoding="utf-8"))
        pages[path.resolve()] = page
        if page.h1_count != 1:
            errors.append(f"{path.name}: expected one h1, found {page.h1_count}")
        if page.duplicate_ids or page.images_without_alt:
            errors.append(f"{path.name}: duplicate IDs or missing image descriptions")
        if not page.description:
            errors.append(f"{path.name}: missing description")
    index = (site / "index.html").resolve()
    if index not in pages:
        errors.append("Missing index.html")
    elif pages[index].canonical != BASE_URL:
        errors.append("Canonical URL does not match the intended GitHub Pages address")
    for path, page in pages.items():
        for reference in page.references:
            url = urlsplit(reference)
            if url.scheme or url.netloc:
                # Verify same-site absolute metadata URLs too, including the share image.
                if not reference.startswith(BASE_URL):
                    continue
                relative = urlsplit(reference[len(BASE_URL):])
                target = site / unquote(relative.path)
            elif url.path.startswith("/"):
                errors.append(f"{path.name}: root-relative URL breaks project hosting: {reference}")
                continue
            else:
                target = path.parent / unquote(url.path) if url.path else path
            if target.is_dir():
                target = target / "index.html"
            target = target.resolve()
            if not target.is_relative_to(site.resolve()) or not target.exists():
                errors.append(f"{path.name}: missing or out-of-site target: {reference}")
            elif url.fragment and target in pages and unquote(url.fragment) not in pages[target].ids:
                errors.append(f"{path.name}: missing anchor: {reference}")
    try:
        sitemap = ET.parse(site / "sitemap.xml")
        locations = [item.text for item in sitemap.findall(".//{http://www.sitemaps.org/schemas/sitemap/0.9}loc")]
        if locations != [BASE_URL]:
            errors.append("Sitemap does not match the canonical site URL")
        snapshot = json.loads((site / "assets/snapshot.json").read_text(encoding="utf-8"))
        for name in SNAPSHOT_ASSETS:
            path = site / "assets" / name
            actual = hashlib.sha256(path.read_bytes()).hexdigest()
            if actual != snapshot["rendered_assets"][name]:
                errors.append(f"{name}: bytes do not match the recorded showcase snapshot")
        rendering = json.loads((site / "assets/rendering.json").read_text(encoding="utf-8"))
        if rendering["pcb_sha256"] != snapshot["pcb_sha256"]:
            errors.append("Rendering metadata describes a different PCB")
        if rendering["all_layers"] != snapshot["copper_layers"] or rendering["views"]["routing-all.svg"]["copper_fills"]:
            errors.append("Combined board view must show the full copper stack with fills hidden")
        if rendering["native_geometry"]["footprints"] != snapshot["component_count"] or rendering["native_geometry"]["pads"] <= 0:
            errors.append("Rendering metadata is missing the recorded footprints/pads")
        for path in (site / "assets").glob("*.svg"):
            ET.parse(path)
        for path in (site / "assets").glob("*.json"):
            json.loads(path.read_text(encoding="utf-8"))
        for match in re.finditer(r'<script type="application/ld\+json">(.*?)</script>', (site / "index.html").read_text(encoding="utf-8"), re.S):
            json.loads(match.group(1))
        if not (site / "assets/social-preview.png").read_bytes().startswith(b"\x89PNG\r\n\x1a\n"):
            errors.append("Social preview is not a PNG")
    except (OSError, ValueError, KeyError, ET.ParseError) as error:
        errors.append(str(error))
    if site.resolve() == SITE.resolve():
        errors.extend(check_copies())
    return errors


if __name__ == "__main__":
    failures = check_site(Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else SITE)
    if failures:
        print("\n".join(f"ERROR: {failure}" for failure in failures))
        raise SystemExit(1)
    print("Static site OK: local URLs, anchors, metadata, sitemap, JSON, SVG and snapshot hashes.")
