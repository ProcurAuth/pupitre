# SPDX-License-Identifier: MIT OR Apache-2.0
# A throwaway image for the test of the callback plugin with a real ansible-playbook.
# It is built on the machine of the developer and never pushed. The script run.sh gives the
# base image (pinned by digest) and the version of ansible-core.
ARG BASE
FROM ${BASE}
ARG ANSIBLE_CORE
RUN apt-get update \
    && apt-get install --no-install-recommends --yes openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir "ansible-core==${ANSIBLE_CORE}"
