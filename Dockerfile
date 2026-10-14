FROM rust:1-alpine AS build

RUN apk add --no-cache musl-dev cmake make perl clang-dev g++ linux-headers git

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
ENV RUSTFLAGS="-C target-feature=-crt-static"
RUN cargo rustc --release --locked --bin idlebot -- -C target-feature=+crt-static \
    && cp target/release/idlebot /idlebot

FROM busybox:musl

COPY --from=build /idlebot /usr/local/bin/idlebot

ENTRYPOINT ["idlebot"]
