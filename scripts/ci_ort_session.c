/* Synthetic ONNX Runtime smoke: dynamic API 24, model load and one CPU run. */
#include <dlfcn.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include "onnxruntime_c_api.h"

static const OrtApi *api;
static void check(OrtStatus *status) {
    if (status) {
        fprintf(stderr, "ONNX Runtime: %s\n", api->GetErrorMessage(status));
        api->ReleaseStatus(status);
        exit(1);
    }
}
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!library) { fprintf(stderr, "%s\n", dlerror()); return 3; }
    const OrtApiBase *(*get_base)(void) = dlsym(library, "OrtGetApiBase");
    if (!get_base) return 4;
    const OrtApiBase *base = get_base();
    if (!base || !(api = base->GetApi(24))) return 5;
    OrtEnv *env = NULL;
    OrtSessionOptions *options = NULL;
    OrtSession *session = NULL;
    OrtMemoryInfo *memory = NULL;
    OrtAllocator *allocator = NULL;
    check(api->CreateEnv(ORT_LOGGING_LEVEL_WARNING, "vrg-ci", &env));
    check(api->CreateSessionOptions(&options));
    check(api->CreateSession(env, argv[2], options, &session));
    check(api->CreateCpuMemoryInfo(OrtArenaAllocator, OrtMemTypeDefault, &memory));
    check(api->GetAllocatorWithDefaultOptions(&allocator));
    size_t count = 0, output_count = 0;
    check(api->SessionGetInputCount(session, &count));
    check(api->SessionGetOutputCount(session, &output_count));
    if (!count || count > 8 || !output_count) return 6;
    char *names[8] = {0};
    const OrtValue *values[8] = {0};
    float *buffers[8] = {0};
    for (size_t i = 0; i < count; i++) {
        OrtTypeInfo *type = NULL;
        const OrtTensorTypeAndShapeInfo *tensor = NULL;
        ONNXTensorElementDataType element;
        size_t ndims = 0, elements = 1;
        int64_t dims[8];
        check(api->SessionGetInputName(session, i, allocator, &names[i]));
        check(api->SessionGetInputTypeInfo(session, i, &type));
        check(api->CastTypeInfoToTensorInfo(type, &tensor));
        check(api->GetTensorElementType(tensor, &element));
        check(api->GetDimensionsCount(tensor, &ndims));
        if (element != ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT || ndims > 8) return 7;
        check(api->GetDimensions(tensor, dims, ndims));
        for (size_t d = 0; d < ndims; d++) {
            if (dims[d] <= 0) dims[d] = 1;
            if (dims[d] > 1024 || elements > 1024 / (size_t)dims[d]) return 8;
            elements *= (size_t)dims[d];
        }
        buffers[i] = calloc(elements, sizeof(float));
        if (!buffers[i]) return 9;
        for (size_t j = 0; j < elements; j++) buffers[i][j] = 1.0f;
        OrtValue *value = NULL;
        check(api->CreateTensorWithDataAsOrtValue(memory, buffers[i], elements * sizeof(float), dims, ndims, element, &value));
        values[i] = value;
        api->ReleaseTypeInfo(type);
    }
    char *output_name = NULL;
    OrtValue *output = NULL;
    check(api->SessionGetOutputName(session, 0, allocator, &output_name));
    const char *output_names[] = {output_name};
    check(api->Run(session, NULL, (const char *const *)names, values, count, output_names, 1, &output));
    float *output_values = NULL;
    check(api->GetTensorMutableData(output, (void **)&output_values));
    if (!output_values || !isfinite(output_values[0])) return 10;
    puts("PASS: API 24, model session and synthetic CPU inference");
    api->ReleaseValue(output);
    for (size_t i = 0; i < count; i++) { api->ReleaseValue((OrtValue *)values[i]); allocator->Free(allocator, names[i]); free(buffers[i]); }
    allocator->Free(allocator, output_name);
    api->ReleaseMemoryInfo(memory);
    api->ReleaseSession(session);
    api->ReleaseSessionOptions(options);
    api->ReleaseEnv(env);
    return 0;
}
