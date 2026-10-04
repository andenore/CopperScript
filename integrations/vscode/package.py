"""Package already staged shared editor assets as a deterministic offline VSIX."""
import argparse
import json
from pathlib import Path
from xml.sax.saxutils import escape
import zipfile


def package(stage, output):
    stage, output = Path(stage), Path(output)
    manifest = json.loads((stage / "package.json").read_text(encoding="utf-8"))
    content_types = b'''<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="json" ContentType="application/json"/>
<Default Extension="xml" ContentType="text/xml"/>
<Default Extension="vsixmanifest" ContentType="text/xml"/>
<Default Extension="cjs" ContentType="application/javascript"/>
<Default Extension="js" ContentType="application/javascript"/>
<Default Extension="html" ContentType="text/html"/>
<Default Extension="css" ContentType="text/css"/>
<Default Extension="md" ContentType="text/markdown"/>
<Default Extension="" ContentType="text/plain"/>
</Types>'''
    xml = f'''<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
<Metadata><Identity Id="{escape(manifest['name'])}" Version="{escape(manifest['version'])}" Language="en-US" Publisher="{escape(manifest['publisher'])}"/>
<DisplayName>{escape(manifest['displayName'])}</DisplayName><Description xml:space="preserve">{escape(manifest['description'])}</Description><License>extension/LICENSE</License></Metadata>
<Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation><Dependencies/>
<Prerequisites><Prerequisite Id="Microsoft.VisualStudio.Code" Version="[1.90.0,)" DisplayName="Visual Studio Code"/></Prerequisites>
<Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets>
</PackageManifest>'''.encode()
    files = {"[Content_Types].xml": content_types, "extension.vsixmanifest": xml}
    for name in ("package.json", "extension.cjs", "README.md", "LICENSE", "media/index.html", "media/editor.js", "media/editor.css"):
        files["extension/" + name] = (stage / name).read_bytes()
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("xb") as stream, zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, raw in sorted(files.items()):
            entry = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.create_system = 3
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, raw)
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", type=Path, default=Path("build/vscode-extension/extension"))
    parser.add_argument("-o", "--output", type=Path, required=True)
    args = parser.parse_args()
    print(package(args.stage, args.output))
