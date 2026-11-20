# syntax=docker/dockerfile:1
# --- construction : binaire statique (musl), interface web comprise ---
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --release --locked

# --- image finale : minuscule ---
FROM alpine:3.20
RUN apk add --no-cache tini ca-certificates
COPY --from=build /src/target/release/kesal-rangeur /usr/local/bin/kesal-rangeur
ENV KESAL_CONFIG=/config/config.toml \
    KESAL_BIND=0.0.0.0:3050
WORKDIR /config
VOLUME /config
EXPOSE 3050
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD wget -qO- http://127.0.0.1:3050/healthz >/dev/null || exit 1
ENTRYPOINT ["/sbin/tini", "--", "kesal-rangeur"]
CMD ["serve"]
