# FASE_2.md — Jardinero: limpieza deliberada en entorno aislado

## Activación obligatoria

Requiere `INFORME_AUDITORIA_REAL.md` de fase 1, revisión por el usuario y una matriz de eliminación aprobada con rutas concretas. No interpretar una recomendación del informe como autorización automática. La fase 2 puede dejar la copia de desarrollo NO FUNCIONAL: no se persigue paridad, compilación ni tests aquí.

## Qué sí hacer

1. Crear nuevo repo VRG si aún no existe; material de referencia y backup de los componentes autorizados.
2. Preparar branch/worktree aislado con alcance y ruta verificables; nunca efectuar operaciones destructivas en fuente original.
3. Traducir la matriz aprobada en ownership de carpetas para agentes independientes (8+ si existe trabajo real).
4. Retirar exclusivamente archivos, dependencias, adaptadores, scripts y configuraciones marcados `ELIMINAR` y aprobados.
5. Preservar componentes `MANTENER` y referencias necesarias para `ADAPTAR`, sin intentar reparar compatibilidad por iniciativa propia.
6. Conservar documentación, decisiones, historial mínimo y trazabilidad de eliminaciones.
7. Registrar lista `eliminado/conservado/bloqueado` y ruta de recuperación/restauración.
8. Dejar estructura limpia y lista de contratos pendientes para fase 3 sin crear implementaciones ficticias.

## Qué no hacer

- No tocar notas originales ni hacer migración de datos.
- No borrar repositorios fuente, respaldos o backups de terceros.
- No `git clean`, `reset --hard` ni operaciones recursivas sobre rutas compartidas.
- No inferir `ELIMINAR` de que un archivo parezca antiguo: usar la matriz aprobada.
- No compilar, probar, arreglar errores de ejecución ni implementar módulos nuevos (eso corresponde a fases posteriores).
- No crear stubs para aparentar funcionalidad ni emplear pruebas exploratorias.

## Paralelismo

Particionar limpieza por grupos de archivos disjuntos. Cada agente produce un change-set identificado y un listado exacto de archivos afectados. El coordinador integra cambios y preserva trabajo preexistente. Un solo agente puede editar un archivo compartido, los demás entregan instrucciones, no parches competidores.

## Entregables

- Worktree/branch aislado con limpieza aprobada.
- `REPORTE_JARDINERO.md` que enumera rutas removidas, justificación/evidencia, rutas conservadas, pendientes y restauración.
- Resumen de contratos/interfaces aún necesarios en fase 3.
- Estado declarado `LIMPIEZA_COMPLETA_SIN_CERTIFICAR` o bloqueado.

## Fin

La fase termina cuando **solo** las eliminaciones aprobadas se han realizado en el lugar permitido y la trazabilidad existe. No perseguir un build verde. No activar fase 3 hasta que se congelen las decisiones arquitectónicas y pruebas propuestas.
