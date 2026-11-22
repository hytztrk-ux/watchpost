# syntax=docker/dockerfile:1

# ---- build ----------------------------------------------------------------
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev gcc
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY watchpost.example.toml ./
COPY src ./src
RUN cargo build --release --locked \
 && mkdir -p /out/data \
 && cp target/release/watchpost /out/watchpost

# ---- runtime: just the static binary ----------------------------------------
FROM scratch
COPY --from=build /out/watchpost /watchpost
COPY --from=build --chown=65532:65532 /out/data /data
USER 65532:65532
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8787
ENTRYPOINT ["/watchpost"]
CMD ["--config", "/data/watchpost.toml", "run"]
