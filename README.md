# Varg Agent - Multi-Soul Matrix AI Agent

A self-contained, self-extending AI chat agent written in [Varg](https://github.com/LupusMalusDeviant/VARG), communicating over the Matrix protocol. Features a **Multi-Soul personality system**, knowledge-graph-driven tool discovery, permanent memory, speech-to-text, sandboxed code execution, and the ability to build its own tools at runtime.

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
                        |   Agent   |      |    STT    |      |  Sandbox  |
                        | (Varg/    |      | (Speech   |      | (Code     |
                        |  Rose)    |      |  to Text) |      |  Exec)    |
                        +-----------+      +-----------+      +-----------+
```

### Components

| Service | Description |
|---------|-------------|
| **agent** | The Varg agent binary. Multi-soul personality, LLM integration, tool system, Graph-RAG |
| **conduit** | Matrix homeserver (Conduit) - lightweight, single-binary Rust server |
| **element** | Element Web - Matrix chat UI |
| **caddy** | Reverse proxy routing Matrix API + Element Web |
| **stt** | Speech-to-text sidecar (Flask + Gemini) for voice message transcription |
| **sandbox** | Code execution sidecar with Docker-in-Docker for running arbitrary code |

## Features

### Multi-Soul System
- Swappable AI personalities stored in the knowledge graph
- Each soul has 5 sections: **identity**, **tone**, **traits**, **empathy**, **user_model**
- Souls bound per Matrix room - different rooms can have different personalities
- Default soul: **Rose** (inspired by Rose Tyler + Alan Turing)
- Create new souls via chat commands: `!soul create`, `!soul set`, `!soul bind`

### Knowledge Graph (4 Domains)
1. **souls** - Personality system (always fully loaded, highest priority)
2. **kg_tools** - Tool registry with vector search (discovers relevant tools per message)
3. **kg_self** - Varg language knowledge, architecture, agent source code
4. **per-room memory** - Episodic memory + vector embeddings + graph DB

### Self-Extending Tools
The agent can write, compile, and register new tools in Varg at runtime:
```
User: "Build me a tool that converts celsius to fahrenheit"
Rose: *writes Varg code, compiles it, registers in knowledge graph*
Rose: "Done! Tool 'celsius_to_f' is ready. Try it!"
```

### Permanent Memory
- Every message (user + agent) stored in memory, graph, and vector store
- Context-based retrieval via Graph-RAG on every message
- `!clear` only resets prompt history - the agent **never forgets**

### Multi-LLM Support
- **Gemini** (default), **OpenAI**, **Anthropic Claude**, **Ollama** (local + cloud)
- Configurable via environment variables

### Speech-to-Text
- Voice messages (m.audio) automatically transcribed via Gemini
- Transcription injected as `[Sprachnachricht] <text>`

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
OPENAI_API_KEY=                    # For OpenAI or Ollama Cloud
ANTHROPIC_API_KEY=                 # For Claude

# Ollama (local or cloud)
VARG_LLM_URL=                     # e.g. http://host.docker.internal:11434 or https://api.ollama.com/v1/chat/completions

# Ports
HTTP_PORT=2077                     # Port for Caddy (Element Web + Matrix API)
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
| `!status` | Agent status, active soul, LLM info |
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
| `!time` | Current timestamp |

## Project Structure

```
Varg-Agent/
  agent/
    agent.varg          # Main agent source (~2200 lines of Varg)
    Dockerfile          # Multi-stage: compile Varg -> Rust -> binary
  stt/
    app.py              # Flask STT sidecar (Gemini transcription)
    Dockerfile
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

[Varg](https://github.com/LupusMalusDeviant/VARG) is a compiled AI agent language that transpiles to Rust. It provides built-in primitives for:

- HTTP requests, JSON parsing
- Graph databases, vector stores, embeddings
- Episodic memory with semantic search
- File I/O, process execution
- Ownership/borrowing (Rust-like)

The agent is 100% written in Varg - no Python, no JavaScript, no glue code for the core agent logic.

## License

MIT
