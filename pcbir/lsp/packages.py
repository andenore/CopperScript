"""Locked dependency snapshots scoped to editor invalidation events.

The normal resolver verifies the complete module inventory once. Subpackages
reuse that verified root until a dependency buffer or watched file changes.
This class is never selected by the build CLI.
"""
from ..packages import PackageResolver, ResolvedPackage
from ..syntax import CopperScriptError


class SnapshotResolver(PackageResolver):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.modules = {}
        self.relative = {}

    def resolve(self, import_path, location):
        if import_path.startswith(('./', '../')):
            key = (import_path, location.filename)
            if key not in self.relative:
                self.relative[key] = super().resolve(import_path, location)
            return self.relative[key]
        module = self._matching_module(import_path)
        if module is None:
            return super().resolve(import_path, location)
        if module not in self.modules:
            self.modules[module] = super().resolve(module, location)
        root = self.modules[module]
        directory = (root.directory / import_path[len(module):].lstrip('/')).resolve()
        if not directory.is_dir() or not directory.is_relative_to(root.directory):
            raise CopperScriptError('PKG004', f'package {import_path!r} does not exist in module {module!r}', location)
        return ResolvedPackage(import_path, root.module_path, root.version, directory, root.checksum)
