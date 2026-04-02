from __future__ import annotations

import importlib
import json
import os
import sys
import types
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import mock

from fastapi import HTTPException


class FakeBrowser:
    instances = []

    def __init__(self, **kwargs):
        self.kwargs = kwargs
        self.closed = False
        self.page = FakePage()
        type(self).instances.append(self)

    async def close(self) -> None:
        self.closed = True
        return None

    async def get_state(self):
        if self.closed:
            raise RuntimeError("browser has been closed")
        return {"url": "https://example.com/current"}

    def is_closed(self):
        return self.closed


class FakePage:
    def __init__(self):
        self.closed = False

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

    def is_closed(self):
        return self.closed


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
    run_outcomes = []

    def __init__(self, task, llm, browser, use_vision):
        self.task = task
        self.llm = llm
        self.browser = browser
        self.use_vision = use_vision
        type(self).instances.append(self)

    async def run(self):
        if type(self).run_outcomes:
            outcome = type(self).run_outcomes.pop(0)
            if isinstance(outcome, Exception):
                raise outcome
            return outcome
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
    FakeAgent.run_outcomes = []
    FakeBrowser.instances = []

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
            self.assertEqual(
                payload["profile_scope_mode"], "runtime_injected_preferred"
            )
            self.assertEqual(payload["max_profiles_per_scope"], 3)
            self.assertEqual(payload["profile_idle_ttl_secs"], 604800)
            self.assertEqual(payload["browser_ready_retries"], 2)
            self.assertEqual(payload["browser_ready_retry_delay_ms"], 750)
            self.assertTrue(payload["browser_ready_retry_supported"])
            self.assertTrue(payload["browser_runtime_observability_supported"])
            self.assertTrue(payload["orphan_profile_recovery_supported"])
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

    async def test_health_reports_browser_ready_retry_overrides(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "",
                    "BROWSER_USE_BRIDGE_BROWSER_READY_RETRIES": "5",
                    "BROWSER_USE_BRIDGE_BROWSER_READY_RETRY_DELAY_MS": "1500",
                }
            )

            response = await module.health()
            payload = json.loads(response.body)

            self.assertEqual(payload["browser_ready_retries"], 5)
            self.assertEqual(payload["browser_ready_retry_delay_ms"], 1500)

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
                browser_llm_config={
                    "provider": "zai",
                    "model": "glm-5-turbo",
                    "api_base": "https://api.z.ai/api/coding/paas/v4/chat/completions",
                    "supports_vision": False,
                },
            )

            response = await manager.run_task(request, "zai-secret")

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.llm_source, "request_config")
            self.assertEqual(response.llm_provider, "zai")
            self.assertEqual(response.llm_transport, "openai_compatible")
            self.assertEqual(response.vision_mode, "disabled")
            self.assertTrue(response.browser_runtime_alive)
            self.assertIsNotNone(response.browser_runtime_last_check_at)
            self.assertIsNone(response.browser_runtime_dead_reason)
            self.assertEqual(FakeAgent.instances[-1].use_vision, False)
            self.assertIsInstance(FakeAgent.instances[-1].llm, FakeChatOpenAI)
            self.assertEqual(
                FakeAgent.instances[-1].llm.kwargs["base_url"],
                "https://api.z.ai/api/coding/paas/v4",
            )
            self.assertEqual(
                FakeAgent.instances[-1].llm.kwargs["api_key"], "zai-secret"
            )
            self.assertTrue(
                FakeAgent.instances[-1].llm.kwargs["dont_force_structured_output"]
            )
            self.assertTrue(
                FakeAgent.instances[-1].llm.kwargs["add_schema_to_system_prompt"]
            )
            self.assertTrue(
                FakeAgent.instances[-1].llm.kwargs["remove_defaults_from_schema"]
            )
            self.assertTrue(
                FakeAgent.instances[-1].llm.kwargs["remove_min_items_from_schema"]
            )

    async def test_google_request_config_does_not_apply_openai_schema_compat_preset(
        self,
    ):
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
                browser_llm_config={
                    "provider": "google",
                    "model": "gemini-2.5-flash",
                    "supports_vision": True,
                },
            )

            response = await manager.run_task(request, None)

            self.assertEqual(response.status, "completed")
            self.assertIsInstance(FakeAgent.instances[-1].llm, FakeChatGoogle)
            self.assertNotIn(
                "dont_force_structured_output", FakeAgent.instances[-1].llm.kwargs
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
            self.assertTrue(response.browser_runtime_alive)
            self.assertIsNotNone(response.browser_runtime_last_check_at)
            self.assertIsNone(response.browser_runtime_dead_reason)
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

    async def test_extract_content_reports_dead_browser_session_as_terminal_error(self):
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
            session = await manager.get_session(run_response.session_id)
            session.browser.closed = True
            session.browser.page = None

            with self.assertRaises(HTTPException) as context:
                await manager.extract_content(
                    run_response.session_id,
                    module.ExtractContentRequest(format="text"),
                )

            self.assertEqual(context.exception.status_code, 409)
            self.assertEqual(
                context.exception.detail["error"], "browser_session_not_alive"
            )
            refreshed = await manager.get_session(run_response.session_id)
            self.assertIsNone(refreshed.browser)
            self.assertFalse(refreshed.browser_runtime_alive)
            self.assertIsNotNone(refreshed.browser_runtime_last_check_at)
            self.assertIn(
                "browser runtime is closed", refreshed.browser_runtime_dead_reason
            )
            self.assertIn("browser runtime is closed", refreshed.last_error)

    async def test_screenshot_reports_dead_browser_session_as_terminal_error(self):
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
            session = await manager.get_session(run_response.session_id)
            session.browser.closed = True
            session.browser.page = None

            with self.assertRaises(HTTPException) as context:
                await manager.screenshot(
                    run_response.session_id,
                    module.ScreenshotRequest(full_page=False),
                )

            self.assertEqual(context.exception.status_code, 409)
            self.assertEqual(
                context.exception.detail["error"], "browser_session_not_alive"
            )
            refreshed = await manager.get_session(run_response.session_id)
            self.assertIsNone(refreshed.browser)
            self.assertFalse(refreshed.browser_runtime_alive)
            self.assertIsNotNone(refreshed.browser_runtime_last_check_at)
            self.assertIn(
                "browser runtime is closed", refreshed.browser_runtime_dead_reason
            )

    async def test_get_session_endpoint_reports_browser_runtime_observability(self):
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
            module.manager = manager

            response = await module.get_session(run_response.session_id)

            self.assertEqual(response.session_id, run_response.session_id)
            self.assertEqual(response.status, "completed")
            self.assertTrue(response.browser_runtime_alive)
            self.assertIsNotNone(response.browser_runtime_last_check_at)
            self.assertIsNone(response.browser_runtime_dead_reason)

    async def test_run_task_creates_and_returns_profile_metadata(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage and summarize it", reuse_profile=True
                ),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertIsNotNone(response.profile_id)
            self.assertEqual(response.profile_scope, "bridge_local")
            self.assertEqual(response.profile_status, "active")
            self.assertTrue(response.profile_attached)
            self.assertFalse(response.profile_reused)
            self.assertIn("user_data_dir", FakeBrowser.instances[-1].kwargs)
            profile_root = Path(tmpdir) / "profiles" / response.profile_id
            self.assertTrue((profile_root / "metadata.json").exists())
            self.assertTrue((profile_root / "browser").exists())

    async def test_run_task_creates_runtime_scoped_profile_metadata(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage and summarize it",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )

            self.assertEqual(response.profile_scope, "topic-a")
            metadata = json.loads(
                (
                    (Path(tmpdir) / "profiles" / response.profile_id) / "metadata.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(metadata["profile_scope"], "topic-a")

    async def test_run_task_reuses_profile_on_new_session(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            first = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage", reuse_profile=True),
                None,
            )
            await manager.close_session(first.session_id)

            second = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the docs page",
                    profile_id=first.profile_id,
                ),
                None,
            )

            self.assertEqual(second.status, "completed")
            self.assertEqual(second.profile_id, first.profile_id)
            self.assertTrue(second.profile_reused)
            self.assertEqual(second.profile_status, "active")
            self.assertTrue(second.profile_attached)
            self.assertEqual(
                FakeBrowser.instances[-1].kwargs.get("user_data_dir"),
                str(Path(tmpdir) / "profiles" / first.profile_id / "browser"),
            )

    async def test_creating_new_profile_keeps_other_live_profile_active(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=2)

            first = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )
            await manager.run_task(
                module.RunTaskRequest(
                    task="Open the docs page",
                    reuse_profile=True,
                    profile_scope="topic-b",
                ),
                None,
            )

            first_metadata = json.loads(
                (
                    Path(tmpdir) / "profiles" / first.profile_id / "metadata.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(first_metadata["status"], "active")
            self.assertEqual(first_metadata["current_session_id"], first.session_id)

    async def test_detaching_profile_keeps_other_live_profile_active(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=2)

            first = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )
            second = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the docs page",
                    reuse_profile=True,
                    profile_scope="topic-b",
                ),
                None,
            )
            await manager.close_session(first.session_id)

            first_metadata = json.loads(
                (
                    Path(tmpdir) / "profiles" / first.profile_id / "metadata.json"
                ).read_text(encoding="utf-8")
            )
            second_metadata = json.loads(
                (
                    Path(tmpdir) / "profiles" / second.profile_id / "metadata.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(first_metadata["status"], "idle")
            self.assertIsNone(first_metadata["current_session_id"])
            self.assertEqual(second_metadata["status"], "active")
            self.assertEqual(second_metadata["current_session_id"], second.session_id)

    async def test_run_task_retries_transient_browser_readiness_error(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(
                Path(tmpdir),
                max_concurrent_sessions=1,
                browser_ready_retries=1,
                browser_ready_retry_delay_ms=0,
            )
            FakeAgent.run_outcomes = [
                RuntimeError("CDP client not initialized"),
                "Recovered after retry",
            ]

            response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.summary, "Recovered after retry")
            self.assertIsNone(response.error)
            self.assertEqual(len(FakeAgent.instances), 2)
            self.assertEqual(len(FakeBrowser.instances), 2)

    async def test_run_task_does_not_retry_non_readiness_error(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(
                Path(tmpdir),
                max_concurrent_sessions=1,
                browser_ready_retries=2,
                browser_ready_retry_delay_ms=0,
            )
            FakeAgent.run_outcomes = [RuntimeError("selector not found")]

            response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"),
                None,
            )

            self.assertEqual(response.status, "failed")
            self.assertIn("selector not found", response.error)
            self.assertEqual(len(FakeAgent.instances), 1)
            self.assertEqual(len(FakeBrowser.instances), 1)

    async def test_run_task_fails_after_transient_retry_budget_exhausted(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(
                Path(tmpdir),
                max_concurrent_sessions=1,
                browser_ready_retries=1,
                browser_ready_retry_delay_ms=0,
            )
            FakeAgent.run_outcomes = [
                RuntimeError("CDP client not initialized"),
                RuntimeError("CDP client not initialized"),
            ]

            response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"),
                None,
            )

            self.assertEqual(response.status, "failed")
            self.assertIn("CDP client not initialized", response.error)
            self.assertEqual(len(FakeAgent.instances), 2)
            self.assertEqual(len(FakeBrowser.instances), 2)

    async def test_close_session_detaches_profile_without_deleting_it(self):
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
                module.RunTaskRequest(task="Open the homepage", reuse_profile=True),
                None,
            )
            close_response = await manager.close_session(run_response.session_id)

            self.assertEqual(close_response.status, "closed")
            self.assertEqual(close_response.profile_id, run_response.profile_id)
            self.assertEqual(close_response.profile_status, "idle")
            self.assertFalse(close_response.profile_attached)
            self.assertFalse(close_response.browser_runtime_alive)
            self.assertIsNotNone(close_response.browser_runtime_last_check_at)
            self.assertEqual(
                close_response.browser_runtime_dead_reason,
                "browser session was closed by bridge",
            )

            session = await manager.get_session(run_response.session_id)
            self.assertEqual(session.profile_id, run_response.profile_id)
            self.assertEqual(session.profile_status, "idle")
            self.assertFalse(session.profile_attached)
            self.assertFalse(session.browser_runtime_alive)

            metadata = json.loads(
                (
                    Path(tmpdir)
                    / "profiles"
                    / run_response.profile_id
                    / "metadata.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(metadata["status"], "idle")
            self.assertIsNone(metadata["current_session_id"])

    async def test_shutdown_detaches_profile_without_deleting_it(self):
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
                module.RunTaskRequest(task="Open the homepage", reuse_profile=True),
                None,
            )
            await manager.shutdown()

            metadata = json.loads(
                (
                    Path(tmpdir)
                    / "profiles"
                    / run_response.profile_id
                    / "metadata.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(metadata["status"], "idle")
            self.assertIsNone(metadata["current_session_id"])

    async def test_run_task_recovers_orphaned_profile_after_restart(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            first_manager = module.SessionManager(
                Path(tmpdir), max_concurrent_sessions=1
            )

            first = await first_manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )

            metadata_path = (
                Path(tmpdir) / "profiles" / first.profile_id / "metadata.json"
            )
            initial_metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
            self.assertEqual(initial_metadata["status"], "active")
            self.assertEqual(initial_metadata["current_session_id"], first.session_id)

            restarted_manager = module.SessionManager(
                Path(tmpdir), max_concurrent_sessions=1
            )
            second = await restarted_manager.run_task(
                module.RunTaskRequest(
                    task="Open the docs page",
                    profile_id=first.profile_id,
                    profile_scope="topic-a",
                ),
                None,
            )

            self.assertEqual(second.status, "completed")
            self.assertEqual(second.profile_id, first.profile_id)
            self.assertTrue(second.profile_reused)

            healed_metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
            self.assertEqual(healed_metadata["status"], "active")
            self.assertEqual(healed_metadata["current_session_id"], second.session_id)

    async def test_run_task_rejects_cross_scope_profile_reuse(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            first = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )
            await manager.close_session(first.session_id)

            response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open docs",
                    profile_id=first.profile_id,
                    profile_scope="topic-b",
                ),
                None,
            )

            self.assertEqual(response.status, "failed")
            self.assertIn("belongs to scope 'topic-a'", response.error)

    async def test_profile_scope_quota_rejects_extra_retained_profiles(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(
                Path(tmpdir), max_concurrent_sessions=1, max_profiles_per_scope=1
            )

            first = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )
            await manager.close_session(first.session_id)

            response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the docs page",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )

            self.assertEqual(response.status, "failed")
            self.assertIn("already has 1 retained profiles", response.error)

    async def test_profile_ttl_prunes_idle_profile_and_frees_quota(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                    "BROWSER_USE_BRIDGE_PROFILE_IDLE_TTL_SECS": "60",
                }
            )
            manager = module.SessionManager(
                Path(tmpdir),
                max_concurrent_sessions=1,
                max_profiles_per_scope=1,
                profile_idle_ttl_secs=60,
            )

            first = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the homepage",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )
            await manager.close_session(first.session_id)

            metadata_path = (
                Path(tmpdir) / "profiles" / first.profile_id / "metadata.json"
            )
            metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
            metadata["updated_at"] = "2000-01-01T00:00:00+00:00"
            metadata["last_used_at"] = "2000-01-01T00:00:00+00:00"
            metadata_path.write_text(
                json.dumps(metadata, ensure_ascii=True, indent=2),
                encoding="utf-8",
            )

            second = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the docs page",
                    reuse_profile=True,
                    profile_scope="topic-a",
                ),
                None,
            )

            self.assertEqual(second.status, "completed")
            self.assertNotEqual(second.profile_id, first.profile_id)
            self.assertFalse(metadata_path.exists())


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
