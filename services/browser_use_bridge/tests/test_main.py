from __future__ import annotations

import importlib
import json
import os
import sys
import types
import unittest
import warnings
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import mock


warnings.filterwarnings(
    "ignore",
    message=r".*on_event is deprecated.*",
    category=DeprecationWarning,
)


class FakeBrowser:
    def __init__(self):
        self.page = FakePage()

    async def close(self) -> None:
        return None

    async def get_state(self):
        return {"url": "https://example.com/current"}


class FakePage:
    async def content(self):
        return "<html><body><h1>Example</h1></body></html>"

    async def evaluate(self, script):
        if "innerText" in script:
            return "Example page text"
        if "outerHTML" in script:
            return "<html><body>Evaluated HTML</body></html>"
        return ""

    async def screenshot(self, path, full_page=False):
        Path(path).write_bytes(b"fake-png")
        return None


class _BaseChat:
    def __init__(self, **kwargs):
        self.kwargs = kwargs


class FakeChatBrowserUse(_BaseChat):
    pass


class FakeChatGoogle(_BaseChat):
    pass


class FakeChatAnthropic(_BaseChat):
    pass


class FakeChatOpenAI(_BaseChat):
    pass


class FakeAgent:
    instances = []

    def __init__(self, task, llm, browser, use_vision):
        self.task = task
        self.llm = llm
        self.browser = browser
        self.use_vision = use_vision
        type(self).instances.append(self)

    async def run(self):
        return "Task completed from fake agent"


def make_browser_use_module() -> types.ModuleType:
    module = types.ModuleType("browser_use")
    module.Agent = FakeAgent
    module.Browser = FakeBrowser
    module.ChatBrowserUse = FakeChatBrowserUse
    module.ChatGoogle = FakeChatGoogle
    module.ChatAnthropic = FakeChatAnthropic
    module.ChatOpenAI = FakeChatOpenAI
    return module


def import_bridge_module(extra_env: dict[str, str]):
    module_name = "services.browser_use_bridge.app.main"
    sys.modules.pop(module_name, None)
    FakeAgent.instances = []

    browser_use_stub = make_browser_use_module()
    with mock.patch.dict(sys.modules, {"browser_use": browser_use_stub}):
        with mock.patch.dict(os.environ, extra_env, clear=False):
            return importlib.import_module(module_name)


class BrowserUseBridgeTests(unittest.IsolatedAsyncioTestCase):
    async def test_health_reports_request_mode_by_default(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "",
                }
            )

            response = await module.health()
            payload = json.loads(response.body)

            self.assertEqual(response.status_code, 200)
            self.assertEqual(
                payload["preferred_browser_llm_source"],
                "request_browser_llm_config",
            )
            self.assertFalse(payload["legacy_env_fallback_configured"])
            self.assertIn("minimax", payload["supported_inherited_route_providers"])
            self.assertIn("browser_use", payload["supported_legacy_env_providers"])

    async def test_health_reports_configured_legacy_fallback(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )

            response = await module.health()
            payload = json.loads(response.body)

            self.assertTrue(payload["legacy_env_fallback_configured"])
            self.assertEqual(payload["legacy_env_llm_provider"], "google")
            self.assertEqual(payload["legacy_env_llm_model"], "gemini-2.5-flash")

    async def test_request_level_run_reports_observability_fields(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            request = module.RunTaskRequest(
                task="Open the docs page and summarize it",
                browser_llm_config=module.BrowserLlmConfig(
                    provider="zai",
                    model="glm-5-turbo",
                    api_base="https://api.z.ai/api/coding/paas/v4/chat/completions",
                    supports_vision=False,
                ),
            )

            response = await manager.run_task(request, "zai-secret")

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.llm_source, "request_config")
            self.assertEqual(response.llm_provider, "zai")
            self.assertEqual(response.llm_transport, "openai_compatible")
            self.assertEqual(response.vision_mode, "disabled")
            self.assertEqual(FakeAgent.instances[-1].use_vision, False)
            self.assertIsInstance(FakeAgent.instances[-1].llm, FakeChatOpenAI)
            self.assertEqual(
                FakeAgent.instances[-1].llm.kwargs["base_url"],
                "https://api.z.ai/api/coding/paas/v4",
            )
            self.assertEqual(
                FakeAgent.instances[-1].llm.kwargs["api_key"], "zai-secret"
            )

    async def test_legacy_fallback_run_reports_observability_fields(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            request = module.RunTaskRequest(task="Open the homepage and summarize it")

            response = await manager.run_task(request, None)

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.llm_source, "legacy_env")
            self.assertEqual(response.llm_provider, "google")
            self.assertEqual(response.llm_transport, "google")
            self.assertEqual(response.vision_mode, "auto")
            self.assertEqual(FakeAgent.instances[-1].use_vision, "auto")
            self.assertIsInstance(FakeAgent.instances[-1].llm, FakeChatGoogle)
            self.assertEqual(
                FakeAgent.instances[-1].llm.kwargs["model"], "gemini-2.5-flash"
            )

    async def test_extract_content_reads_active_session_page(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            run_response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"), None
            )

            response = await manager.extract_content(
                run_response.session_id,
                module.ExtractContentRequest(format="text", max_chars=7),
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.format, "text")
            self.assertEqual(response.content, "Example")
            self.assertTrue(response.truncated)
            self.assertEqual(response.total_chars, len("Example page text"))
            self.assertEqual(response.current_url, "https://example.com/current")

    async def test_screenshot_persists_artifact_for_active_session(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            run_response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"), None
            )

            response = await manager.screenshot(
                run_response.session_id,
                module.ScreenshotRequest(full_page=True),
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.artifact["kind"], "screenshot")
            self.assertTrue(response.artifact["full_page"])
            self.assertTrue(Path(response.artifact["path"]).exists())
            self.assertGreater(response.artifact["size_bytes"], 0)
            session = await manager.get_session(run_response.session_id)
            self.assertEqual(len(session.artifacts), 1)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
