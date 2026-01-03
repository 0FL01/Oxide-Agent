#!/bin/bash

set -e

echo "Building image: ${DOCKER_IMAGE}:${SHA_SHORT}"
docker build --pull \
  --label "org.opencontainers.image.source=${CI_REPOSITORY_URL}" \
  --label "org.opencontainers.image.revision=${CI_COMMIT_SHA}" \
  -t "${DOCKER_IMAGE}:${SHA_SHORT}" .

if [ "$CI_PIPELINE_SOURCE" != "merge_request_event" ]; then
  echo "Pushing image: ${DOCKER_IMAGE}:${SHA_SHORT}"
  docker push "${DOCKER_IMAGE}:${SHA_SHORT}"
else
  echo "Skipping push for merge request."
fi