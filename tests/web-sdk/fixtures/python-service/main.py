import unicodedata

from cordis_sdk import Component, run

import text_services as bindings


class Normalizer:
    def normalize(self, value):
        return unicodedata.normalize("NFKC", value)


async def apply(context, config):
    await context.provide("text-normalizer@1.0", Normalizer(), bindings)


run("web-python-service", [Component("normalizer", apply, provides=("text-normalizer@1.0",))])
