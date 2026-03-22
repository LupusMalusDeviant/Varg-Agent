# Sandbox Sidecar - Code execution, git cloning, Varg compilation
FROM python:3.11-slim-bookworm

# Install Docker CLI + git
RUN apt-get update && apt-get install -y \
    git \
    curl \
    gnupg \
    && curl -fsSL https://download.docker.com/linux/debian/gpg | \
       gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg \
    && echo "deb [arch=amd64 signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] \
       https://download.docker.com/linux/debian bookworm stable" \
       > /etc/apt/sources.list.d/docker.list \
    && apt-get update \
    && apt-get install -y docker-ce-cli \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY app.py .

RUN mkdir -p /tmp/repos

EXPOSE 5001

CMD ["python", "app.py"]
