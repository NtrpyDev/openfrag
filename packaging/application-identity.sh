#!/usr/bin/env bash
# Defines the immutable application and package artifact names shared by OpenFrag Linux tooling.

OPENFRAG_APPLICATION_ID=io.github.ntrpydev.openfrag
OPENFRAG_DESKTOP_NAME=${OPENFRAG_APPLICATION_ID}.desktop
OPENFRAG_LAUNCHER_NAME=openfrag-launch
OPENFRAG_DAEMON_NAME=openfragd
OPENFRAG_USER_SERVICE=app-${OPENFRAG_APPLICATION_ID}.service

readonly \
    OPENFRAG_APPLICATION_ID \
    OPENFRAG_DESKTOP_NAME \
    OPENFRAG_LAUNCHER_NAME \
    OPENFRAG_DAEMON_NAME \
    OPENFRAG_USER_SERVICE
