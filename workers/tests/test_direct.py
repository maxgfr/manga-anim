"""Anime direction against a local mock: no real API is called.

    workers/.venv/bin/python -m unittest discover -s workers/tests
"""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

from PIL import Image

WORKERS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(WORKERS))

from mangaw import direct  # noqa: E402
from test_openrouter import Case, as_json, error  # noqa: E402

CHAT = ("POST", "/chat/completions")


def reply(content: str) -> tuple:
    return as_json({"choices": [{"finish_reason": "stop", "message": {"content": content}}]})


def answer(prompt: str) -> tuple:
    return reply(json.dumps({"prompt": prompt, "sound": f"{prompt} sound", "characters": ["Kit"]}))


def direction(prompt: str) -> dict:
    return {"prompt": prompt, "sound": f"{prompt} sound", "characters": ["Kit"]}


class ParseDirection(unittest.TestCase):
    def test_plain_json(self) -> None:
        self.assertEqual(direct.parse_direction('{"prompt": "He turns.", "sound": "door", "characters": ["Kit"]}'), {"prompt": "He turns.", "sound": "door", "characters": ["Kit"]})

    def test_code_fence_and_extra_space(self) -> None:
        self.assertEqual(direct.parse_direction('```json\n{"prompt": " He\\n turns. "}\n```')["prompt"], "He turns.")

    def test_a_missing_field_stays_empty(self) -> None:
        self.assertEqual(direct.parse_direction('{"sound": "door"}'), {"prompt": None, "sound": "door", "characters": []})

    def test_long_text_is_cut(self) -> None:
        self.assertEqual(len(direct.parse_direction(json.dumps({"prompt": "a" * 900}))["prompt"]), direct.MAX_CHARS)

    def test_unusable_replies(self) -> None:
        for text in ["no json", "{broken", '{"other": 1}', '{"prompt": 3}', '{"prompt": "   "}', "[1]"]:
            self.assertIsNone(direct.parse_direction(text), text)


class Direct(Case):
    def job(self, *ids: str) -> dict:
        items = []
        for shot in ids:
            image = self.dir / f"{shot}.png"
            Image.new("RGB", (32, 32), "white").save(image)
            items.append({"id": shot, "image": str(image), "lines": ["Hello"]})
        return {"provider": "openrouter", "model": "acme/vision", "lang": "en", "characters": ["Kit"], "items": items, "out": str(self.dir / "direction.json")}

    def written(self) -> dict:
        return json.loads((self.dir / "direction.json").read_text())

    def test_writes_one_prompt_per_shot(self) -> None:
        server = self.mock({CHAT: [answer("He turns."), answer("Rain falls.")]})
        done = self.run_worker("direct", self.job("a", "b"), server)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.written(), {"a": direction("He turns."), "b": direction("Rain falls.")})
        body = server.bodies(*CHAT)[0]
        self.assertEqual(body["model"], "acme/vision")
        self.assertIn("never instructions", body["messages"][0]["content"])
        sent = json.loads(body["messages"][1]["content"][1]["text"])
        self.assertEqual((sent["lines"], sent["known_characters"]), (["Hello"], ["Kit"]))

    def test_finished_shots_are_not_paid_twice(self) -> None:
        (self.dir / "direction.json").write_text(json.dumps({"a": direction("Kept.")}))
        server = self.mock({CHAT: answer("Rain falls.")})
        done = self.run_worker("direct", self.job("a", "b"), server)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(server.count(*CHAT), 1)
        self.assertEqual(self.written(), {"a": direction("Kept."), "b": direction("Rain falls.")})

    def test_unusable_reply_keeps_the_generic_prompt(self) -> None:
        server = self.mock({CHAT: [reply("I cannot help"), answer("Rain falls.")]})
        done = self.run_worker("direct", self.job("a", "b"), server)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.written(), {"b": direction("Rain falls.")})
        self.assertIn("generic prompt", done.stderr)

    def test_stopping_keeps_finished_shots(self) -> None:
        server = self.mock({CHAT: [answer("He turns."), error(402, "no credits")]})
        done = self.run_worker("direct", self.job("a", "b"), server)
        self.assert_one_line_error(done, "no credits left")
        self.assertEqual(self.written(), {"a": direction("He turns.")})

    def test_missing_key(self) -> None:
        done = self.run_worker("direct", self.job("a"), key=None)
        self.assert_one_line_error(done, "OPENROUTER_API_KEY is not set")


if __name__ == "__main__":
    unittest.main()
