FROM python:3.9-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /usr/local/bin/uv

WORKDIR /app

COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev

COPY args.py ./

EXPOSE 8080

ENTRYPOINT ["uv", "run", "python", "args.py"]
CMD ["--mode=default"]
