# syntax=docker/dockerfile:1.3-labs
FROM rust:bullseye AS build

COPY . /src/
WORKDIR /src

RUN --mount=type=cache,target=/usr/local/cargo/registry <<EOF
	set -e
	cargo build --release
	mkdir /output
	mv /src/target/release/linux-mail-db /output/
EOF

FROM bitnami/minideb:bullseye

VOLUME /mail
VOLUME /log
VOLUME /app/config.yaml

EXPOSE 80

COPY --from=build /output/linux-mail-db /app/
WORKDIR /app

CMD [ "./linux-mail-db" ]
