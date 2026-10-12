# GritIRT frontend image: Veloasm console + Tailwind, built by Trunk.
# Build context is the workspace root (see ../docker-compose.yml) so the
# Veloasm path dependency resolves inside the build. Trunk downloads the
# Tailwind standalone binary itself on first use (network required).
ARG TRUNK_VERSION=0.21.14
# Backend URL baked into the WASM bundle (the page calls it from the browser).
ARG IRT_API_URL=http://127.0.0.1:8030
FROM rust:1-bookworm AS builder
ARG TRUNK_VERSION
ARG IRT_API_URL
RUN rustup target add wasm32-unknown-unknown \
    && curl -sSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
        | tar -xz -C /usr/local/bin trunk
WORKDIR /src/GritIRT/frontend
# Framework first for better layer caching.
COPY Velo /src/Velo
COPY GritIRT/frontend ./
RUN sed -i 's|pub const BASE_URL: &str = "[^"]*";|pub const BASE_URL: \&str = "'"${IRT_API_URL}"'";|' src/api.rs \
    && grep -n 'pub const BASE_URL' src/api.rs \
    && trunk build --release

FROM nginx:alpine
COPY GritIRT/frontend/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=builder /src/GritIRT/frontend/dist/ /usr/share/nginx/html/
EXPOSE 80
