# Varg Agent - Multi-Soul Matrix AI Agent

A self-contained, self-extending AI chat agent written in [Varg](https://github.com/LupusMalusDeviant/VARG) (v0.9.0), communicating over the Matrix protocol. Features a **Multi-Soul personality system**, SQLite-persistent knowledge graphs, 37 built-in tools, permanent memory, and the ability to build its own tools at runtime.

## Architecture

```
+------------------+     +------------+     +-----------+
|   Element Web    |<--->|   Caddy    |<--->|  Conduit  |
|   (Chat UI)      |     | (Reverse   |     | (Matrix   |
|                  |     |  Proxy)    |     |  Server)  |
+------------------+     +------------+     +-----------+
                                                  ^
                                                  |
                              +-------------------+-------------------+
                              |                   |                   |
                        +-----------+      +-----------+      +-----------+
                        |   Agent   |      |   Media   |      |  Sandbox  |
                        | (Varg/    |      | (Rust     |      | (Code     |
                        |  Rose)    |      |  Sidecar) |      |  Exec)    |
                        +-----------+      +-----------+      +-----------+
```

### Components

| Service | Description |
|---------|-------------|
| **agent** | The Varg agent binary. Multi-soul personality, LLM integration, 37 tools, Graph-RAG, SQLite-persistent knowledge |
| **conduit** | Matrix homeserver (Conduit) - lightweight, single-binary Rust server |
| **element** | Element Web - Matrix chat UI |
| **caddy** | Reverse proxy routing Matrix API + Element Web |
| **stt** | Rust media sidecar (varg-media) - STT, TTS, web search, email, calendar, image gen, binary uploads |
| **sandbox** | Code execution sidecar with Docker-in-Docker for running arbitrary code |

## Features

### Multi-Soul System
- Swappable AI personalities stored entirely in the knowledge graph (SQLite-persistent)
- Each soul has sections: **identity**, **tone**, **traits**, **empathy**, **user_model**
- Souls bound per Matrix room - different rooms can have different personalities
- Default soul: **Rose** - warm, direct, curious
- Create/modify souls via chat commands or the `update_soul` tool (Rose can modify her own personality)
- Soul modifications persist across container restarts
- `!soul create`, `!soul set`, `!soul bind`

### 37 Built-in Tools

**Files & Memory**
- `read_file` / `write_file` / `send_file` - File system access + Matrix file sharing
- `remember` / `recall` - Room-scoped persistent notes

**Code & Development**
- `run_code` - Execute code in Docker sandboxes (Python, Node, Rust, etc.)
- `git_clone` / `read_repo_file` - Clone and explore Git repositories
- `build_varg_tool` / `run_varg_tool` / `list_tools` - Self-extending tool system

**Web & Search**
- `web_search` - DuckDuckGo web search
- `fetch_url` - Extract text content from URLs

**Media & Documents**
- `generate_pdf` - Create PDF documents with sections and headings
- `analyze_image` - Image analysis via Gemini Vision
- `tts_respond` - Text-to-speech via Gemini TTS
- `generate_image` - Image generation via Gemini Imagen
- `convert_image` - Image format/size conversion
- `convert_to_pdf` - Convert images to PDF

**Communication**
- `send_email` / `list_emails` / `read_email` - IMAP/SMTP email integration
- `set_reminder` - Timed reminders (cycle-based countdown)

**Smart Home**
- `smarthome_list` / `smarthome_status` / `smarthome_control` - Home Assistant integration

**Location & Navigation**
- `search_location` - Geocoding via Nominatim
- `get_route` - Routing via OSRM

**Productivity**
- `add_todo` / `list_todos` / `done_todo` - Per-room todo lists
- `get_time` - Current timestamp
- `get_weather` - Weather via OpenWeatherMap
- `translate` - Translation via Gemini
- `calendar_list` / `calendar_create` - CalDAV calendar integration

**System**
- `set_config` / `list_config` - Configure API keys and credentials via Matrix chat
- `update_soul` - Modify soul personality sections at runtime

### Knowledge Graph (4 Domains, SQLite-Persistent)

All knowledge graphs are automatically persisted to SQLite via Varg v0.9.0. Data survives container restarts.

1. **souls** - Personality system (always fully loaded, highest priority)
2. **kg_tools** - Tool registry with vector search (discovers relevant tools per message)
3. **kg_own** - Varg language knowledge, architecture, agent source code
4. **per-room memory** - Episodic memory + vector embeddings + graph DB

Version-gated seeding: the graph is only seeded on first run or after a version bump (`!reseed` to force).

### Self-Extending Tools
The agent can write, compile, and register new tools in Varg at runtime:
```
User: "Build me a tool that converts celsius to fahrenheit"
Rose: *writes Varg code, compiles it, registers in knowledge graph*
Rose: "Done! Tool 'celsius_to_f' is ready. Try it!"
```

### Permanent Memory
- Every message (user + agent) stored in memory, graph, and vector store
- All stored in SQLite - survives restarts, never lost
- Context-based retrieval via Graph-RAG on every message
- `!clear` only resets prompt history - the agent **never forgets**

### Multi-LLM Support
- **Gemini** (default), **OpenAI**, **Anthropic Claude**, **Ollama** (local + cloud)
- Configurable via environment variables

### Media Sidecar (Rust)
A single Rust binary handling binary/media operations that require non-UTF-8 data:
- Speech-to-text (Gemini transcription)
- Text-to-speech (Gemini TTS)
- Image generation (Gemini Imagen)
- Binary file uploads to Matrix (PDF, images, audio)
- Web search (DuckDuckGo HTML scraping)
- URL text extraction (scraper)
- Email send/list/read (lettre + IMAP)
- Calendar integration (CalDAV)
- Image conversion (image crate)

### Sandboxed Code Execution
- Run arbitrary code in Docker containers
- Clone Git repos and explore them
- Default image: `python:3.11-slim`, configurable per request

## Installation

### Prerequisites

- Linux server (Debian/Ubuntu recommended)
- Docker + Docker Compose v2
- 2GB+ RAM, 10GB+ disk
- (Optional) Domain name + reverse proxy for HTTPS

### Quick Start

```bash
# 1. Clone the repository
git clone https://github.com/LupusMalusDeviant/Varg-Agent.git
cd Varg-Agent

# 2. Create your .env file
cp deploy/.env.example deploy/.env
nano deploy/.env  # Edit with your settings

# 3. Build and start everything
cd deploy
docker compose up --build -d

# 4. Watch the logs
docker compose logs -f agent
```

### Environment Variables

Create `deploy/.env` with the following:

```env
# Matrix Server
SERVER_NAME=localhost              # Your domain or IP

# Agent Account (auto-registered on first start)
MATRIX_USERNAME=rose               # Matrix username for the agent
MATRIX_PASSWORD=YourSecurePassword # Matrix password
AGENT_DISPLAY_NAME=Rose            # Display name in chat

# LLM Provider (choose one: gemini, openai, anthropic, ollama)
VARG_LLM_PROVIDER=gemini
VARG_LLM_MODEL=gemini-2.0-flash

# API Keys (only the one for your chosen provider)
GEMINI_API_KEY=your-gemini-key
OPENAI_API_KEY=                    # For OpenAI
ANTHROPIC_API_KEY=                 # For Claude

# Ollama (local or cloud)
VARG_LLM_URL=                     # e.g. http://host.docker.internal:11434

# Ports
HTTP_PORT=2077                     # Port for Caddy (Element Web + Matrix API)
```

Optional service credentials (can also be set via Matrix chat using `set_config`):

```env
# Weather
OPENWEATHERMAP_API_KEY=            # For get_weather tool

# Smart Home
HASS_URL=                          # Home Assistant URL
HASS_TOKEN=                        # Home Assistant long-lived access token

# Email
SMTP_HOST=                         # SMTP server
SMTP_PORT=587
SMTP_USER=
SMTP_PASS=
SMTP_FROM=                         # Sender email address
IMAP_HOST=                         # IMAP server
IMAP_PORT=993
IMAP_USER=
IMAP_PASS=

# Calendar (CalDAV)
CALDAV_URL=                        # CalDAV server URL
CALDAV_USER=
CALDAV_PASS=
```

### Accessing the Chat

1. Open `http://your-server:2077` in your browser (Element Web)
2. Register a user account on the homeserver
3. Start a DM with `@rose:your-server-name` (or whatever you set as `MATRIX_USERNAME`)
4. The agent auto-joins invited rooms

### HTTPS Setup (Optional)

For external access with SSL, put a reverse proxy (Nginx Proxy Manager, Caddy, etc.) in front of port 2077 with your domain. The Caddy inside the stack handles Matrix API routing + Element Web.

Example with external Caddy:
```
matrix.yourdomain.com {
    reverse_proxy localhost:2077
}
```

Then update `deploy/conduit/conduit.toml`:
```toml
server_name = "matrix.yourdomain.com"
```

And `deploy/element/config.json`:
```json
{
    "default_server_config": {
        "m.homeserver": {
            "base_url": "https://matrix.yourdomain.com",
            "server_name": "matrix.yourdomain.com"
        }
    }
}
```

## Chat Commands

| Command | Description |
|---------|-------------|
| `!help` | Show all commands |
| `!status` | Agent status, active soul, LLM info, KG version |
| `!soul` | Show active soul for this room |
| `!soul list` | List all available souls |
| `!soul create <name>` | Create a new empty soul |
| `!soul bind <name>` | Bind a soul to the current room |
| `!soul info <name>` | Show all sections of a soul |
| `!soul set <name> <section> <text>` | Set a soul section (identity/tone/traits/empathy/user_model) |
| `!tools` | List all registered tools (from knowledge graph) |
| `!remember <text>` | Store a note in room context |
| `!recall` | Retrieve stored notes |
| `!clear` | Clear prompt history (memory stays permanent) |
| `!reseed` | Reset knowledge graph version (reseed on next restart) |
| `!time` | Current timestamp |

## Project Structure

```
Varg-Agent/
  agent/
    agent.varg          # Main agent source (~3800 lines of Varg)
    Dockerfile          # Multi-stage: compile Varg -> Rust -> binary
  stt/
    src/main.rs         # Rust media sidecar (15 endpoints)
    Cargo.toml          # varg-media crate
    Dockerfile          # Multi-stage Rust build
  sandbox/
    app.py              # Flask sandbox sidecar (Docker-in-Docker)
    Dockerfile
  deploy/
    docker-compose.yml  # Full stack definition
    conduit/            # Conduit homeserver config
    element/            # Element Web config
    caddy/              # Caddy reverse proxy config
```

## The Varg Language

[Varg](https://github.com/LupusMalusDeviant/VARG) (v0.9.0) is a compiled AI agent language that transpiles to Rust. It provides built-in primitives for:

- HTTP requests, JSON parsing
- Graph databases with SQLite persistence
- Vector stores with SQLite persistence
- Episodic memory with semantic search (persistent)
- Base64 encoding/decoding (native)
- PDF generation (native, via printpdf)
- File I/O, process execution
- Ownership/borrowing (Rust-like)

The agent is 100% written in Varg - no Python, no JavaScript, no glue code for the core agent logic. Binary operations (audio, images, Matrix media uploads) are handled by a Rust media sidecar since Varg's HTTP layer is string-only.

## License

MIT
