"""Unlimited-OCR on vLLM: one engine per process, whole batches per call.

vLLM decodes every image of a batch together and captures CUDA graphs, so
the GPU stays busy instead of waiting on Python between tokens (the HF
`model.infer` path runs one image at a time at ~30% GPU utilisation).

The recipe follows the model card and vLLM's unlimited_ocr module: prompt
starting with a literal <image>, no special-token skipping, and the
no-repeat-ngram logits processor with its per-request window.
"""

import os

import attr

REPO = "baidu/Unlimited-OCR"
REVISION = "07dea832e22aefee32ad281d4b80551282e1c168"
PROMPT = "<image>document parsing."
MAX_TOKENS = 8192
NGRAM = {"ngram_size": 35, "window_size": 128}


@attr.s(auto_attribs=True)
class VllmOcr:
    # Leaves room on the 46 GB L40S for the CLIP image encoder in the same
    # worker process.
    gpu_memory_utilization: float = 0.6
    llm: object = attr.ib(init=False, default=None)
    params: object = attr.ib(init=False, default=None)

    def __attrs_post_init__(self):
        # The model's no-repeat-ngram processor implements vLLM's v1
        # logits-processor interface, which the v2 GPU model runner (the
        # default in 0.31) refuses; the v1 runner still takes it.
        os.environ.setdefault("VLLM_USE_V2_MODEL_RUNNER", "0")
        # vLLM 0.31's DeepEncoder attention kernel reads LOG2E, a plain float,
        # as a Triton global, and Triton 3.7 (torch 2.13's) only accepts
        # constexpr globals. Running the engine in this process lets the
        # rebinding below reach the kernel before it first compiles.
        os.environ.setdefault("VLLM_ENABLE_V1_MULTIPROCESSING", "0")
        # Decoding is greedy, so FlashInfer's sampler buys nothing and would
        # JIT-compile CUDA on first use, which needs a toolkit the nodes lack.
        os.environ.setdefault("VLLM_USE_FLASHINFER_SAMPLER", "0")
        import vllm.model_executor.models.deepencoder as deepencoder
        from vllm import LLM, SamplingParams
        from vllm.model_executor.models.unlimited_ocr import NGramPerReqLogitsProcessor
        from vllm.triton_utils import tl

        if not isinstance(deepencoder.LOG2E, tl.constexpr):
            deepencoder.LOG2E = tl.constexpr(deepencoder.LOG2E)

        self.llm = LLM(
            model=REPO,
            revision=REVISION,
            trust_remote_code=True,
            logits_processors=[NGramPerReqLogitsProcessor],
            enable_prefix_caching=False,
            mm_processor_cache_gb=0,
            gpu_memory_utilization=self.gpu_memory_utilization,
        )
        self.params = SamplingParams(
            temperature=0.0,
            max_tokens=MAX_TOKENS,
            skip_special_tokens=False,
            extra_args=NGRAM,
        )

    def __call__(self, images):
        """Raw grounded output for each PIL image, in order."""
        requests = [{"prompt": PROMPT, "multi_modal_data": {"image": image.convert("RGB")}} for image in images]
        outputs = self.llm.generate(requests, self.params, use_tqdm=False)
        return [output.outputs[0].text for output in outputs]
