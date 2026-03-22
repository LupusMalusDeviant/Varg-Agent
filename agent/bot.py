#!/usr/bin/env python3
"""
Varg Matrix Agent - Python bridge implementation.
Connects to Matrix via nio, routes messages to LLM (Gemini/OpenAI/Anthropic/Ollama),
with tool calling, knowledge management, and self-extending capabilities.
"""

import asyncio
import json
import os
import re
import time
import logging
from collections import defaultdict

from nio import (
    AsyncClient,
    LoginResponse,
    RoomMessageText,
    InviteMemberEvent,
    SyncResponse,
)

logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(levelname)s] %(message)s")
log = logging.getLogger("varg-agent")

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

HOMESERVER = os.environ.get("MATRIX_HOMESERVER", "http://conduit:6167")
USERNAME = os.environ.get("MATRIX_USERNAME", "varg-agent")
PASSWORD = os.environ.get("MATRIX_PASSWORD", "changeme")
DISPLAY_NAME = os.environ.get("AGENT_DISPLAY_NAME", "Varg Agent")
AUTO_JOIN = os.environ.get("AGENT_AUTO_JOIN", "true").lower() == "true"

LLM_PROVIDER = os.environ.get("VARG_LLM_PROVIDER", "gemini")
LLM_MODEL = os.environ.get("VARG_LLM_MODEL", "")
GEMINI_API_KEY = os.environ.get("GEMINI_API_KEY", "")
OPENAI_API_KEY = os.environ.get("OPENAI_API_KEY", "")
ANTHROPIC_API_KEY = os.environ.get("ANTHROPIC_API_KEY", "")
OLLAMA_URL = os.environ.get("VARG_LLM_URL", "http://127.0.0.1:11434")

SYSTEM_PROMPT = os.environ.get("AGENT_SYSTEM_PROMPT", """You are a Varg Agent - an AI assistant running on the Matrix protocol.
You are helpful, concise, and knowledgeable. You can store and recall knowledge.

Available tools:
- add_knowledge: Store a fact (subject, predicate, object)
- search_knowledge: Search stored knowledge by query
- knowledge_stats: Show knowledge store statistics

To use a tool: <tool_call>{"name": "tool_name", "arguments": {"key": "value"}}</tool_call>
""")

# ---------------------------------------------------------------------------
# Knowledge Store (in-memory graph + search)
# ---------------------------------------------------------------------------

class KnowledgeStore:
    def __init__(self):
        self.facts: list[dict] = []

    def add(self, subject: str, predicate: str, obj: str) -> str:
        fact = {"subject": subject, "predicate": predicate, "object": obj, "ts": time.time()}
        self.facts.append(fact)
        return f"Stored: {subject} --{predicate}--> {obj}"

    def search(self, query: str, top_k: int = 5) -> str:
        query_lower = query.lower()
        scored = []
        for f in self.facts:
            text = f"{f['subject']} {f['predicate']} {f['object']}".lower()
            score = sum(1 for w in query_lower.split() if w in text)
            if score > 0:
                scored.append((score, f))
        scored.sort(key=lambda x: -x[0])
        results = scored[:top_k]
        if not results:
            return "No relevant knowledge found."
        lines = [f"- {r['subject']} {r['predicate']} {r['object']}" for _, r in results]
        return f"Found {len(lines)} results:\n" + "\n".join(lines)

    def stats(self) -> str:
        return f"Knowledge store: {len(self.facts)} facts"


knowledge = KnowledgeStore()

# ---------------------------------------------------------------------------
# Conversation Manager
# ---------------------------------------------------------------------------

class ConversationManager:
    def __init__(self, max_history: int = 20):
        self.histories: dict[str, list[dict]] = defaultdict(list)
        self.max_history = max_history

    def add(self, room_id: str, role: str, content: str):
        self.histories[room_id].append({"role": role, "content": content})
        if len(self.histories[room_id]) > self.max_history:
            self.histories[room_id] = self.histories[room_id][-self.max_history:]

    def get_messages(self, room_id: str) -> list[dict]:
        return list(self.histories[room_id])


conversations = ConversationManager()

# ---------------------------------------------------------------------------
# Tool System
# ---------------------------------------------------------------------------

TOOL_CALL_RE = re.compile(r"<tool_call>(.*?)</tool_call>", re.DOTALL)


def execute_tool(name: str, args: dict) -> str:
    if name == "add_knowledge":
        return knowledge.add(args.get("subject", ""), args.get("predicate", ""), args.get("object", ""))
    elif name == "search_knowledge":
        return knowledge.search(args.get("query", ""), int(args.get("top_k", 5)))
    elif name == "knowledge_stats":
        return knowledge.stats()
    else:
        return f"Unknown tool: {name}"


def parse_tool_calls(text: str) -> list[tuple[str, dict]]:
    calls = []
    for match in TOOL_CALL_RE.finditer(text):
        try:
            data = json.loads(match.group(1).strip())
            calls.append((data.get("name", ""), data.get("arguments", {})))
        except json.JSONDecodeError:
            pass
    return calls


def strip_tool_calls(text: str) -> str:
    return TOOL_CALL_RE.sub("", text).strip()

# ---------------------------------------------------------------------------
# LLM Providers
# ---------------------------------------------------------------------------

async def llm_chat(messages: list[dict], system: str) -> str:
    provider = LLM_PROVIDER.lower()

    if provider == "gemini":
        return await _gemini_chat(messages, system)
    elif provider == "openai":
        return await _openai_chat(messages, system)
    elif provider == "anthropic":
        return await _anthropic_chat(messages, system)
    elif provider == "ollama":
        return await _ollama_chat(messages, system)
    else:
        return f"Unknown LLM provider: {provider}"


async def _gemini_chat(messages: list[dict], system: str) -> str:
    import aiohttp

    model = LLM_MODEL or "gemini-2.0-flash"
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={GEMINI_API_KEY}"

    contents = []
    for msg in messages:
        role = "model" if msg["role"] == "assistant" else "user"
        contents.append({"role": role, "parts": [{"text": msg["content"]}]})

    body = {"contents": contents}
    if system:
        body["systemInstruction"] = {"parts": [{"text": system}]}

    async with aiohttp.ClientSession() as session:
        async with session.post(url, json=body) as resp:
            data = await resp.json()
            try:
                return data["candidates"][0]["content"]["parts"][0]["text"]
            except (KeyError, IndexError):
                log.error(f"Gemini error: {json.dumps(data)[:500]}")
                return f"LLM error: {data.get('error', {}).get('message', 'Unknown error')}"


async def _openai_chat(messages: list[dict], system: str) -> str:
    from openai import AsyncOpenAI

    client = AsyncOpenAI(api_key=OPENAI_API_KEY)
    model = LLM_MODEL or "gpt-4o"
    msgs = [{"role": "system", "content": system}] + messages if system else messages

    resp = await client.chat.completions.create(model=model, messages=msgs)
    return resp.choices[0].message.content or ""


async def _anthropic_chat(messages: list[dict], system: str) -> str:
    from anthropic import AsyncAnthropic

    client = AsyncAnthropic(api_key=ANTHROPIC_API_KEY)
    model = LLM_MODEL or "claude-sonnet-4-20250514"

    resp = await client.messages.create(
        model=model,
        max_tokens=4096,
        system=system or "You are a helpful assistant.",
        messages=messages,
    )
    return resp.content[0].text


async def _ollama_chat(messages: list[dict], system: str) -> str:
    import aiohttp

    model = LLM_MODEL or "llama3"
    url = f"{OLLAMA_URL}/api/chat"
    msgs = [{"role": "system", "content": system}] + messages if system else messages

    body = {"model": model, "messages": msgs, "stream": False}

    async with aiohttp.ClientSession() as session:
        async with session.post(url, json=body) as resp:
            data = await resp.json()
            return data.get("message", {}).get("content", "")

# ---------------------------------------------------------------------------
# Matrix Bot
# ---------------------------------------------------------------------------

class VargMatrixBot:
    def __init__(self):
        self.client: AsyncClient | None = None
        self.startup_sync_done = False

    async def start(self):
        log.info(f"Connecting to {HOMESERVER} as {USERNAME}...")
        self.client = AsyncClient(HOMESERVER, f"@{USERNAME}:{HOMESERVER.split('//')[1].split(':')[0]}")

        resp = await self.client.login(PASSWORD, device_name="Varg Agent")
        if not isinstance(resp, LoginResponse):
            log.error(f"Login failed: {resp}")
            return

        log.info(f"Logged in as {resp.user_id}")

        # Set display name
        try:
            await self.client.set_displayname(DISPLAY_NAME)
        except Exception as e:
            log.warning(f"Could not set display name: {e}")

        # Register callbacks
        self.client.add_event_callback(self._on_message, RoomMessageText)
        if AUTO_JOIN:
            self.client.add_event_callback(self._on_invite, InviteMemberEvent)

        # Initial sync to skip historical messages
        log.info("Running initial sync...")
        await self.client.sync(timeout=10000)
        self.startup_sync_done = True
        log.info("Initial sync done. Listening for messages...")

        # Sync forever
        await self.client.sync_forever(timeout=30000)

    async def _on_invite(self, room, event):
        if event.membership == "invite" and event.state_key == self.client.user_id:
            log.info(f"Invited to {room.room_id}, joining...")
            await self.client.join(room.room_id)

    async def _on_message(self, room, event):
        if not self.startup_sync_done:
            return
        if event.sender == self.client.user_id:
            return

        body = event.body
        if not body:
            return

        log.info(f"[{room.room_id}] {event.sender}: {body}")

        # Typing indicator
        try:
            await self.client.room_typing(room.room_id, True, timeout=30000)
        except Exception:
            pass

        # Add to conversation
        conversations.add(room.room_id, "user", body)

        # Build knowledge context
        knowledge_ctx = knowledge.search(body, 3)
        system = SYSTEM_PROMPT
        if knowledge_ctx and "No relevant" not in knowledge_ctx:
            system += f"\n\nRelevant knowledge:\n{knowledge_ctx}"

        # Call LLM with tool loop
        messages = conversations.get_messages(room.room_id)
        response = await llm_chat(messages, system)

        max_loops = 5
        loop = 0
        while parse_tool_calls(response) and loop < max_loops:
            tool_calls = parse_tool_calls(response)
            results = []
            for name, args in tool_calls:
                log.info(f"Executing tool: {name}({args})")
                result = execute_tool(name, args)
                results.append(f"Tool '{name}': {result}")

            text_part = strip_tool_calls(response)
            if text_part:
                conversations.add(room.room_id, "assistant", text_part)

            tool_result_text = "\n".join(results)
            conversations.add(room.room_id, "user", f"Tool results:\n{tool_result_text}")

            messages = conversations.get_messages(room.room_id)
            response = await llm_chat(messages, system)
            loop += 1

        final = strip_tool_calls(response) or response
        conversations.add(room.room_id, "assistant", final)

        # Stop typing
        try:
            await self.client.room_typing(room.room_id, False)
        except Exception:
            pass

        # Send response
        await self.client.room_send(
            room.room_id,
            "m.room.message",
            {"msgtype": "m.text", "body": final},
        )
        log.info(f"[{room.room_id}] Agent: {final[:100]}...")


async def main():
    bot = VargMatrixBot()
    await bot.start()


if __name__ == "__main__":
    asyncio.run(main())
