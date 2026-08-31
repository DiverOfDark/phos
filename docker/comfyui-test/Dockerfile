# CPU-only ComfyUI for the contract tests in backend/tests/comfyui_contract_test.rs.
#
# Phos talks to ComfyUI over an API whose payloads are mostly untyped — which
# output key a node publishes under, how the filename counter is spelled, what
# a rejected prompt's 400 body looks like. Unit tests pin *our reading* of that
# contract; this image lets CI pin the contract itself, with workflows built
# from core nodes only, so no model download is ever needed.
#
# CI builds and pushes ghcr.io/<owner>/comfyui-test:<sha256 of this file>,
# so bumping COMFYUI_VERSION here is all it takes to test against a newer
# ComfyUI. Locally: docker build -t comfyui-test docker/comfyui-test
ARG COMFYUI_VERSION=v0.34.2

FROM python:3.13-slim

ARG COMFYUI_VERSION
ENV PIP_ROOT_USER_ACTION=ignore \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    PYTHONUNBUFFERED=1

ADD https://github.com/Comfy-Org/ComfyUI/archive/refs/tags/${COMFYUI_VERSION}.tar.gz /tmp/comfyui.tar.gz
RUN mkdir -p /opt/comfyui \
 && tar -xzf /tmp/comfyui.tar.gz -C /opt/comfyui --strip-components=1 \
 && rm /tmp/comfyui.tar.gz

WORKDIR /opt/comfyui

# CPU wheels first, from PyTorch's own index; the requirements file then finds
# torch already satisfied and does not pull the multi-gigabyte CUDA build.
RUN pip install --no-cache-dir torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cpu \
 && pip install --no-cache-dir -r requirements.txt

EXPOSE 8188
CMD ["python", "main.py", "--cpu", "--listen", "0.0.0.0", "--port", "8188"]
