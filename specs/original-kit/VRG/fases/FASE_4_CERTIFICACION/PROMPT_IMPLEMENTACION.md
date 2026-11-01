# PROMPT_IMPLEMENTACION.md — Ejecutar solo fase 4

Actúa como coordinador de certificación y corrección de VRG. Lee `FASE_4.md`, `GOAL.md`, especificación y **registro de pruebas firmado/aprobado**. No uses catálogo de ejemplos como permiso para ejecutarlos.

## Instrucciones obligatorias

1. Reconoce la revisión candidata y extrae únicamente pruebas cuyo `ID`, comando, datos y fase 4 estén aprobados.
2. Si no hay autorización inequívoca, detente: no crear ni ejecutar ninguna prueba.
3. Corre cada prueba aprobada en una ejecución mínima coordinada por un único runner. Devuelve solo estado resumido, duración si ya está disponible y fallo relevante; nunca miles de líneas.
4. Delega diagnóstico estático de fallos en paralelo a agentes con propiedad de módulo, no a runners paralelos repetitivos.
5. Arregla bugs confirmados sin añadir alcance nuevo. Para verificar correcciones, repite únicamente las pruebas autorizadas aplicables.
6. Completa matriz de fallos por módulo: estado de fallo, bloqueo, errores tipados, reintentos limitados, recuperación segura y reversión. Lo que no puede recuperarse debe fallar de forma explícita y controlada.
7. Señala discrepancias con datos reales sin esconder que una función no se probó.
8. Entrega reporte final completo y detente; no migrar las notas antes de aprobación de fase 5.

## Evitar consumo innecesario

No ejecutar un test por archivo editado, ni `cargo test --workspace` si ese comando exacto no fue aprobado; no benchmarkear, no añadir lint, no simular un despliegue extenso, no pegar logs. Los agentes pueden investigar en paralelo, pero **solo hay un runner de pruebas**.

## Si algo queda bloqueado

Prioriza integridad de notas y seguridad, detén la operación riesgosa, señala el ID de prueba/permiso faltante y no declares `COMPLETE` sin calificaciones. No extender GOAL automáticamente por intentar perseguir 100 % de cobertura.
