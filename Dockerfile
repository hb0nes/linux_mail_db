FROM rust:bullseye AS build
COPY . /src/
WORKDIR /src
RUN cargo build --release
RUN find /src
RUN ls -lrth /src/target/release
COPY /src/target/release/linux-mail-db /output/

FROM bitnami/minideb::bullseye

VOLUME /mail
VOLUME /log
VOLUME /config

EXPOSE 80

COPY --from=build /output/linux-mail-db /app/

CMD [ "/app/linux-mail-db" ]
