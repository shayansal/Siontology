#!/usr/bin/env python3
"""Validates ACP requests and answers against ACP's published JSON Schemas.

    acp_schemas.py <json-schema dir> <samples.json>

<json-schema dir> is spec/<version>/json-schema of the ACP repository
(agentic-commerce-protocol/agentic-commerce-protocol); <samples.json> is
what `cargo test -p sion-cli --test interop -- --ignored` writes: a list
of {name, schema, def, body}. Each body is validated against the named
definition with a JSON Schema 2020-12 validator; a report is printed, and
the exit status is 1 if any sample fails. Needs `jsonschema`,
`referencing` and, for the webhook event (defined in ACP's OpenAPI files
only), `pyyaml`.
"""

from __future__ import annotations

import copy
import json
import pathlib
import sys
from urllib.parse import urljoin

from jsonschema import Draft202012Validator, FormatChecker
from referencing import Registry, Resource


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    schemas = pathlib.Path(sys.argv[1])
    samples = json.loads(pathlib.Path(sys.argv[2]).read_text())
    resources = {}
    for path in sorted(schemas.glob("*.json")):
        document = json.loads(path.read_text())
        resources[path.name] = document
    # Each file is found by its $id, and by its name next to every other
    # file's $id (the files refer to one another by file name).
    entries = []
    for name, doc in resources.items():
        resource = Resource.from_contents(doc)
        entries.append((name, resource))
        if "$id" in doc:
            entries.append((doc["$id"], resource))
        for other in resources.values():
            if "$id" in other:
                entries.append((urljoin(other["$id"], name), resource))
    registry = Registry().with_resources(entries)
    failed = 0
    checked = 0
    for sample in samples:
        if "openapi" in sample:
            # The webhook event is defined in ACP's OpenAPI file only: its
            # envelope from there, its order from the JSON Schema.
            import yaml

            spec = yaml.safe_load((schemas.parent / "openapi" / sample["openapi"]).read_text())
            envelope = copy.deepcopy(spec["components"]["schemas"]["WebhookEvent"])
            data = spec["components"]["schemas"]["EventDataOrder"]
            checkout = resources["schema.agentic_checkout.json"]["$id"]
            envelope["properties"]["data"] = {
                "allOf": [{"$ref": f"{checkout}#/$defs/Order"}],
                "required": data["required"],
            }
            validator = Draft202012Validator(envelope, registry=registry, format_checker=FormatChecker())
            errors = list(validator.iter_errors(sample["body"]))
            checked += 1
            if errors:
                failed += 1
                print(f"FAIL {sample['name']} ({sample['openapi']}#WebhookEvent)")
                for error in errors[:8]:
                    print(f"     {error.message[:300]}")
            else:
                print(f"ok   {sample['name']} (WebhookEvent, from {sample['openapi']})")
            continue
        document = resources[sample["schema"]]
        base = document.get("$id", sample["schema"])
        body = copy.deepcopy(sample["body"])
        notes = []
        # Fields an active extension adds, at the JSONPaths it declares
        # (ACP's extension mechanism), are set aside: the core schema does
        # not know them.
        extensions = body.get("capabilities", {}).get("extensions", []) if isinstance(body, dict) else []
        for declared in extensions if isinstance(extensions, list) else []:
            for path in declared.get("extends", []) if isinstance(declared, dict) else []:
                _, schema_name, field = path.split(".", 2)
                family = sample["def"] == schema_name or (
                    schema_name == "CheckoutSession" and sample["def"].startswith("CheckoutSession")
                    and not sample["def"].endswith("Request")
                )
                if family and field in body:
                    body.pop(field)
                    notes.append(f"extension field {field} ({declared['name']}) set aside")
        def errors_of(definition: str, instance) -> list:
            validator = Draft202012Validator(
                {"$ref": f"{base}#/$defs/{definition}"}, registry=registry, format_checker=FormatChecker()
            )
            return sorted(validator.iter_errors(instance), key=lambda e: list(e.absolute_path))
        if sample["def"] == "DelegateAuthenticationSessionWithResult":
            # ACP's definition is its base (closed: additionalProperties
            # false) and the result, under allOf: no session that has a
            # result can meet it as written. Checked as the base, and the
            # result on its own.
            result = body.pop("authentication_result", None)
            errors = errors_of("DelegateAuthenticationSessionBase", body)
            if result is not None:
                errors += errors_of("AuthenticationResult", result)
            notes.append("ACP's definition cannot be met as written; checked as its base plus its result")
        else:
            errors = errors_of(sample["def"], body)
        checked += 1
        if errors:
            failed += 1
            print(f"FAIL {sample['name']} ({sample['schema']}#{sample['def']})")
            for error in errors[:8]:
                where = "$" + "".join(f"[{p!r}]" if isinstance(p, int) else f".{p}" for p in error.absolute_path)
                print(f"     {where}: {error.message[:300]}")
        else:
            print(f"ok   {sample['name']} ({sample['def']})" + (f"  [{'; '.join(notes)}]" if notes else ""))
    print(f"{checked - failed} of {checked} samples meet the schemas")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
