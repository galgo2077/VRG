# PLANTILLA NUEVA — Informe de auditoría real de VRG

> Esta plantilla se creó durante la preparación de Fase 2 porque la ruta anunciada por el Spec Kit estaba ausente. **No es el original recuperado** y no reproduce contenido desconocido. Su uso futuro requiere confirmar que esta nueva plantilla es aceptable.

## Control del informe

- Proyecto:
- Fase y alcance autorizado:
- Fecha:
- Auditor/coordinador:
- Estado final permitido por la especificación de la fase:
- Fuentes autorizadas consultadas:
- Fuentes excluidas o inaccesibles:

## 1. Resumen ejecutivo

Indicar qué se inspeccionó, qué se verificó, los principales límites de evidencia y la decisión de fase. No presentar hipótesis como hechos.

## 2. Alcance y método

| Actividad | Estado | Alcance exacto | Evidencia |
|---|---|---|---|
| Inventario estático de fuentes | | | |
| Inspección de configuración/documentación | | | |
| Revisión multiagente, si fue autorizada | | | |
| Pruebas autorizadas | | | |

Registrar explícitamente comandos prohibidos o no ejecutados cuando sea relevante. No ejecutar pruebas por el hecho de que esta plantilla las enumere.

## 3. Fuentes y raíces verificadas

| Ruta | Tipo (activa/archivada/generada/documentación) | Evidencia inspeccionada | Límites |
|---|---|---|---|

Separar código real, archivos de configuración, documentos, datos de usuario, artefactos generados y copias. Indicar si una ruta no fue accesible o no fue revisada.

## 4. Arquitectura y dependencias

Describir componentes, límites, datos que consumen/producen y dependencias directas. Citar ruta y símbolo/archivo concretos para cada afirmación técnica.

| Componente | Entrada/salida | Dependencias | Evidencia | Incertidumbre |
|---|---|---|---|---|

## 5. Configuración y discrepancias

| Configuración/ruta | Valor observado | Otra fuente en conflicto | Autoridad demostrada | Estado (verificado/UNKNOWN) |
|---|---|---|---|---|

No declarar autoridad de configuración si solo puede confirmarse ejecutando un servicio que no está autorizado.

## 6. Inventario y clasificación de componentes

Cada elemento debe tener ruta identificable, evidencia y motivo. No clasificar por antigüedad o apariencia.

| Componente/ruta | Clase (KEEP/ADAPT/REPLACE/REMOVE/UNKNOWN) | Justificación verificable | Dependencias/consumidores | Condición o aprobación pendiente |
|---|---|---|---|---|

- **KEEP:** conservar sin cambios en esta fase.
- **ADAPT:** conservar como referencia o adaptar más adelante, sujeto a límites especificados.
- **REPLACE:** sustituir mediante una fase autorizada, sin eliminar antes sus dependencias o datos.
- **REMOVE:** eliminar solo si existe aprobación explícita para la ruta concreta.
- **UNKNOWN:** evidencia insuficiente; registrar la causa exacta y su impacto.

## 7. Documentación y plantillas ausentes

| Referencia anunciada | Ruta esperada | Evidencia de ausencia | Fuente original disponible | Consecuencia |
|---|---|---|---|---|

No reconstruir ni atribuir contenido a documentos faltantes. Si se crea una alternativa nueva, identificarla como nueva y mantenerla separada del original.

## 8. Candidatos de eliminación para una fase posterior

| Ruta exacta | Evidencia de obsolescencia | Dependencias | Riesgo de eliminación | Aprobación necesaria | Recuperación |
|---|---|---|---|---|---|

Si no hay candidatos confirmados, escribirlo expresamente. Una recomendación no equivale a autorización.

## 9. Riesgos de datos y protecciones

Registrar raíces permitidas, exclusiones, enlaces/symlinks, escritura automática, datos fuente, copias, índices derivados, secretos y estrategia de reversión. Separar riesgos confirmados de hipótesis.

| Riesgo | Evidencia | Impacto | Protección requerida | Pendiente |
|---|---|---|---|---|

## 10. Dependencias para la siguiente fase

| Dependencia/decisión | Evidencia actual | Bloquea qué trabajo | Acción mínima requerida | Responsable |
|---|---|---|---|---|

## 11. Lista cerrada de validación propuesta

| ID | Comando/acción exacta | Componente/entrada | Resultado esperado | Riesgo cubierto | Requiere aprobación |
|---|---|---|---|---|---|

La lista es una propuesta, no autorización para ejecutar. Excluir pruebas vagas o abiertas; precisar comandos y datos de entrada antes de solicitar aprobación.

## 12. Evidencia faltante y decisiones abiertas

| Pregunta | Estado (UNKNOWN/pendiente) | Causa exacta | Implicación | Evidencia o aprobación necesaria |
|---|---|---|---|---|

## 13. Decisión y cierre de fase

- Decisión permitida por el documento de fase:
- Requisitos completados con evidencia:
- Requisitos pendientes:
- Cambios realizados (si fueron autorizados):
- Pruebas ejecutadas y evidencia (o “ninguna”):
- Siguiente acción autorizada:

No declarar la fase completa si quedan requisitos obligatorios o bloqueos sin resolver. No comenzar una fase posterior sin su autorización.
