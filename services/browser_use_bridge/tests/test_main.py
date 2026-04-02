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
    initial_start_failures = 0

    def __init__(self, **kwargs):
        self.kwargs = kwargs
        self.browser_profile = types.SimpleNamespace(
            keep_alive=kwargs.get("keep_alive", False)
        )
        self.closed = False
        self.started = False
        self.start_calls = 0
        self.stop_calls = 0
        self.kill_calls = 0
        self.current_url = "https://example.com/current"
        self.current_page = None
        self.session_manager = None
        self._cdp_client_root = None
        self.remaining_start_failures = type(self).initial_start_failures
        type(self).instances.append(self)

    async def start(self) -> None:
        self.start_calls += 1
        if self.closed:
            raise RuntimeError("browser has been closed")
        if self.remaining_start_failures > 0:
            self.remaining_start_failures -= 1
            raise RuntimeError(
                "CDP client not initialized - browser may not be connected yet"
            )
        self.started = True
        self.session_manager = object()
        self._cdp_client_root = object()
        if self.current_page is None:
            self.current_page = FakePage()

    async def close(self) -> None:
        self.kill_calls += 1
        self.closed = True
        self.current_page = None
        self.session_manager = None
        self._cdp_client_root = None
        return None

    async def stop(self) -> None:
        self.stop_calls += 1
        self.session_manager = None
        self._cdp_client_root = None
        self.current_page = None
        return None

    async def kill(self) -> None:
        self.kill_calls += 1
        self.closed = True
        self.current_page = None
        self.session_manager = None
        self._cdp_client_root = None
        return None

    async def get_current_page(self):
        if self.closed:
            raise RuntimeError("browser has been closed")
        return self.current_page

    async def get_current_page_url(self):
        if self.closed:
            raise RuntimeError("browser has been closed")
        if self.session_manager is None:
            raise RuntimeError("SessionManager not initialized")
        return self.current_url

    async def get_browser_state_summary(self, include_screenshot=False):
        if self.closed:
            raise RuntimeError("browser has been closed")
        if self.session_manager is None:
            raise RuntimeError("SessionManager not initialized")
        return {
            "url": self.current_url,
            "text": "Example page text",
            "screenshot": "ZmFrZS1wbmc=" if include_screenshot else None,
        }

    async def get_state_as_text(self):
        if self.closed:
            raise RuntimeError("browser has been closed")
        if self.session_manager is None:
            raise RuntimeError("SessionManager not initialized")
        return "Example page text"

    async def take_screenshot(self, path=None, full_page=False):
        payload = b"fake-png"
        if path is not None:
            Path(path).write_bytes(payload)
        return payload

    def is_closed(self):
        return self.closed


class FakePage:
    def __init__(self):
        self.closed = False

    async def evaluate(self, script):
        if "=>" not in script:
            raise ValueError("JavaScript code must start with (...args) => format")
        if "innerText" in script:
            return "Example page text"
        if "outerHTML" in script:
            return "<html><body>Evaluated HTML</body></html>"
        return ""

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
    auto_close_after_run = False

    def __init__(self, task, llm, browser, use_vision, **kwargs):
        self.task = task
        self.llm = llm
        self.browser = browser
        self.use_vision = use_vision
        self.kwargs = kwargs
        type(self).instances.append(self)

    async def run(self):
        if type(self).run_outcomes:
            outcome = type(self).run_outcomes.pop(0)
            if isinstance(outcome, Exception):
                raise outcome
        else:
            outcome = "Task completed from fake agent"

        if type(self).auto_close_after_run:
            if not getattr(self.browser.browser_profile, "keep_alive", False):
                await self.browser.kill()

        return outcome


class FakeAgentHistory:
    def __init__(
        self,
        *,
        done: bool,
        successful: bool | None,
        final_result_text: str | None = None,
        errors: list[str | None] | None = None,
    ):
        self._done = done
        self._successful = successful
        self._final_result_text = final_result_text
        self._errors = errors or []

    def is_done(self):
        return self._done

    def is_successful(self):
        return self._successful

    def final_result(self):
        return self._final_result_text

    def errors(self):
        return self._errors


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
    FakeAgent.auto_close_after_run = False
    FakeBrowser.instances = []
    FakeBrowser.initial_start_failures = 0

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
            self.assertTrue(payload["execution_mode_split_supported"])
            self.assertTrue(payload["navigation_only_keep_alive_supported"])
            self.assertTrue(payload["browser_runtime_observability_supported"])
            self.assertTrue(payload["browser_keep_alive_observability_supported"])
            self.assertTrue(payload["browser_runtime_reconnect_supported"])
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
            self.assertEqual(response.execution_mode, "autonomous")
            self.assertTrue(response.browser_runtime_alive)
            self.assertIsNotNone(response.browser_runtime_last_check_at)
            self.assertIsNone(response.browser_runtime_dead_reason)
            self.assertFalse(response.browser_keep_alive_requested)
            self.assertFalse(response.browser_keep_alive_effective)
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
            self.assertEqual(response.execution_mode, "autonomous")
            self.assertTrue(response.browser_runtime_alive)
            self.assertIsNotNone(response.browser_runtime_last_check_at)
            self.assertIsNone(response.browser_runtime_dead_reason)
            self.assertFalse(response.browser_keep_alive_requested)
            self.assertFalse(response.browser_keep_alive_effective)
            self.assertEqual(FakeAgent.instances[-1].use_vision, "auto")
            self.assertIsInstance(FakeAgent.instances[-1].llm, FakeChatGoogle)
            self.assertEqual(
                FakeAgent.instances[-1].llm.kwargs["model"], "gemini-2.5-flash"
            )

    async def test_navigation_only_execution_mode_applies_strict_agent_preset(self):
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
                    task="Open the dashboard",
                    execution_mode="navigation_only",
                ),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.execution_mode, "navigation_only")
            self.assertFalse(FakeAgent.instances[-1].kwargs["enable_planning"])
            self.assertFalse(FakeAgent.instances[-1].kwargs["use_judge"])
            self.assertEqual(FakeAgent.instances[-1].kwargs["max_actions_per_step"], 1)
            self.assertIn(
                "This run is navigation-only",
                FakeAgent.instances[-1].kwargs["extend_system_message"],
            )

    async def test_navigation_only_mode_keeps_runtime_alive_for_follow_up_tools(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            FakeAgent.auto_close_after_run = True
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            run_response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the dashboard",
                    execution_mode="navigation_only",
                ),
                None,
            )

            self.assertEqual(run_response.status, "completed")
            self.assertEqual(run_response.execution_mode, "navigation_only")
            self.assertTrue(run_response.browser_runtime_alive)
            self.assertTrue(run_response.browser_keep_alive_requested)
            self.assertTrue(run_response.browser_keep_alive_effective)
            self.assertTrue(FakeBrowser.instances[-1].kwargs["keep_alive"])
            self.assertEqual(FakeBrowser.instances[-1].stop_calls, 0)
            self.assertEqual(FakeBrowser.instances[-1].kill_calls, 0)

            screenshot = await manager.screenshot(
                run_response.session_id,
                module.ScreenshotRequest(full_page=False),
            )

            self.assertEqual(screenshot.status, "completed")

    async def test_navigation_only_mode_keeps_runtime_alive_for_extract_follow_up(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            FakeAgent.auto_close_after_run = True
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            run_response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the dashboard",
                    execution_mode="navigation_only",
                ),
                None,
            )

            response = await manager.extract_content(
                run_response.session_id,
                module.ExtractContentRequest(format="text"),
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.content, "Example page text")

    async def test_navigation_only_follow_up_reconnects_detached_runtime(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            FakeAgent.auto_close_after_run = True
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            run_response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the dashboard",
                    execution_mode="navigation_only",
                ),
                None,
            )
            session = await manager.get_session(run_response.session_id)
            browser = session.browser
            self.assertIsNotNone(browser)
            browser.current_page = None
            browser.session_manager = None
            browser._cdp_client_root = None
            start_calls_before = browser.start_calls

            screenshot = await manager.screenshot(
                run_response.session_id,
                module.ScreenshotRequest(full_page=False),
            )

            self.assertEqual(screenshot.status, "completed")
            refreshed = await manager.get_session(run_response.session_id)
            self.assertTrue(refreshed.browser_runtime_alive)
            self.assertTrue(refreshed.browser_reconnect_attempted)
            self.assertTrue(refreshed.browser_reconnect_succeeded)
            self.assertIsNone(refreshed.browser_reconnect_error)
            self.assertEqual(browser.start_calls, start_calls_before + 1)

    async def test_navigation_only_follow_up_returns_409_when_reconnect_fails(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            FakeAgent.auto_close_after_run = True
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            run_response = await manager.run_task(
                module.RunTaskRequest(
                    task="Open the dashboard",
                    execution_mode="navigation_only",
                ),
                None,
            )
            session = await manager.get_session(run_response.session_id)
            browser = session.browser
            self.assertIsNotNone(browser)
            browser.current_page = None
            browser.session_manager = None
            browser._cdp_client_root = None
            browser.remaining_start_failures = 1

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
            self.assertTrue(refreshed.browser_reconnect_attempted)
            self.assertFalse(refreshed.browser_reconnect_succeeded)
            self.assertIn(
                "CDP client not initialized", refreshed.browser_reconnect_error
            )
            self.assertIn("CDP client not initialized", refreshed.last_error)

    async def test_autonomous_mode_allows_upstream_to_close_runtime(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            FakeAgent.auto_close_after_run = True
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)

            response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.execution_mode, "autonomous")
            self.assertFalse(response.browser_runtime_alive)
            self.assertFalse(response.browser_keep_alive_requested)
            self.assertFalse(response.browser_keep_alive_effective)
            self.assertIn(
                "browser runtime is closed", response.browser_runtime_dead_reason
            )
            self.assertNotIn("keep_alive", FakeBrowser.instances[-1].kwargs)
            self.assertEqual(FakeBrowser.instances[-1].kill_calls, 1)
            self.assertEqual(FakeBrowser.instances[-1].stop_calls, 0)

    async def test_explicit_autonomous_mode_overrides_legacy_steering_wrapper(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            steering_task = (
                "Browser Use execution rules for this run:\n"
                "- Use this step only for navigation and interaction needed to reach the target page or UI state.\n"
                "- Do not take screenshots, save PDFs, or perform final page-content extraction in this step.\n"
                "- Leave the session on the target page for Oxide follow-up tools.\n"
                "- Return a short navigation/status summary only.\n\n"
                "Original task:\nOpen the dashboard and take a screenshot"
            )

            response = await manager.run_task(
                module.RunTaskRequest(
                    task=steering_task,
                    execution_mode="autonomous",
                ),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.execution_mode, "autonomous")
            self.assertEqual(FakeAgent.instances[-1].kwargs, {})

    async def test_legacy_steering_wrapper_still_applies_navigation_only_fallback(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            steering_task = (
                "Browser Use execution rules for this run:\n"
                "- Use this step only for navigation and interaction needed to reach the target page or UI state.\n"
                "- Do not take screenshots, save PDFs, or perform final page-content extraction in this step.\n"
                "- Leave the session on the target page for Oxide follow-up tools.\n"
                "- Return a short navigation/status summary only.\n\n"
                "Original task:\nOpen the dashboard and take a screenshot"
            )

            response = await manager.run_task(
                module.RunTaskRequest(task=steering_task), None
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.execution_mode, "navigation_only")
            self.assertFalse(FakeAgent.instances[-1].kwargs["enable_planning"])

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
            session.browser.current_page = None
            session.browser.session_manager = None
            session.browser._cdp_client_root = None

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
            session.browser.current_page = None
            session.browser.session_manager = None
            session.browser._cdp_client_root = None

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
            self.assertEqual(response.execution_mode, "autonomous")
            self.assertTrue(response.browser_runtime_alive)
            self.assertIsNotNone(response.browser_runtime_last_check_at)
            self.assertIsNone(response.browser_runtime_dead_reason)
            self.assertFalse(response.browser_keep_alive_requested)
            self.assertFalse(response.browser_keep_alive_effective)

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

    async def test_run_task_waits_for_browser_runtime_before_agent_start(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            FakeBrowser.initial_start_failures = 1
            manager = module.SessionManager(
                Path(tmpdir),
                max_concurrent_sessions=1,
                browser_ready_retries=1,
                browser_ready_retry_delay_ms=0,
            )

            response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(len(FakeBrowser.instances), 1)
            self.assertEqual(len(FakeAgent.instances), 1)
            self.assertEqual(FakeBrowser.instances[-1].start_calls, 2)
            self.assertTrue(response.browser_runtime_alive)

    async def test_run_task_reads_final_result_from_browser_history(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            FakeAgent.run_outcomes = [
                FakeAgentHistory(
                    done=True,
                    successful=True,
                    final_result_text="Wikipedia homepage is ready",
                )
            ]

            response = await manager.run_task(
                module.RunTaskRequest(task="Open wikipedia and summarize it"),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.summary, "Wikipedia homepage is ready")

    async def test_run_task_fails_before_agent_start_when_browser_warmup_never_recovers(
        self,
    ):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            FakeBrowser.initial_start_failures = 3
            manager = module.SessionManager(
                Path(tmpdir),
                max_concurrent_sessions=1,
                browser_ready_retries=0,
                browser_ready_retry_delay_ms=0,
            )

            response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"),
                None,
            )

            self.assertEqual(response.status, "failed")
            self.assertIn("CDP client not initialized", response.error)
            self.assertEqual(len(FakeBrowser.instances), 1)
            self.assertEqual(len(FakeAgent.instances), 0)

    async def test_run_task_marks_internal_failed_history_as_failed(self):
        with TemporaryDirectory() as tmpdir:
            module = import_bridge_module(
                {
                    "BROWSER_USE_BRIDGE_DATA_DIR": tmpdir,
                    "BROWSER_USE_BRIDGE_LLM_PROVIDER": "google",
                    "BROWSER_USE_BRIDGE_LLM_MODEL": "gemini-2.5-flash",
                }
            )
            manager = module.SessionManager(Path(tmpdir), max_concurrent_sessions=1)
            FakeAgent.run_outcomes = [
                FakeAgentHistory(
                    done=False,
                    successful=None,
                    errors=[None, "Stopped due to 5 consecutive failures"],
                )
            ]

            response = await manager.run_task(
                module.RunTaskRequest(task="Open example.com and summarize it"),
                None,
            )

            self.assertEqual(response.status, "failed")
            self.assertIn("Stopped due to 5 consecutive failures", response.error)
            self.assertEqual(len(FakeAgent.instances), 1)
            self.assertEqual(len(FakeBrowser.instances), 1)

    async def test_run_task_retries_internal_readiness_failure_history(self):
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
                FakeAgentHistory(
                    done=False,
                    successful=None,
                    errors=[
                        "CDP client not initialized - browser may not be connected yet"
                    ],
                ),
                FakeAgentHistory(
                    done=True,
                    successful=True,
                    final_result_text="Recovered after history retry",
                ),
            ]

            response = await manager.run_task(
                module.RunTaskRequest(task="Open the homepage and summarize it"),
                None,
            )

            self.assertEqual(response.status, "completed")
            self.assertEqual(response.summary, "Recovered after history retry")
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
            self.assertEqual(close_response.execution_mode, "autonomous")
            self.assertFalse(close_response.browser_keep_alive_requested)
            self.assertEqual(close_response.profile_id, run_response.profile_id)
            self.assertEqual(close_response.profile_status, "idle")
            self.assertFalse(close_response.profile_attached)
            self.assertFalse(close_response.browser_runtime_alive)
            self.assertIsNotNone(close_response.browser_runtime_last_check_at)
            self.assertEqual(
                close_response.browser_runtime_dead_reason,
                "browser session was closed by bridge",
            )
            self.assertFalse(close_response.browser_keep_alive_effective)
            self.assertEqual(FakeBrowser.instances[-1].kill_calls, 1)

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
