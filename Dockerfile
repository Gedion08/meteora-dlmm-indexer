FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p dlmm-indexer -p dlmm-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/dlmm-indexer /usr/local/bin/dlmm-indexer
COPY --from=build /src/target/release/dlmm-api /usr/local/bin/dlmm-api
USER 10001
EXPOSE 9100 8080
ENTRYPOINT ["dlmm-indexer"]
