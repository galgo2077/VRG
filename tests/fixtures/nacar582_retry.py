"""Política sintética de espera NACAR582, expresada en segundos."""

import json


def espera_segundos(reintentos: int) -> int:
    """Calcula la espera para un número no negativo de reintentos."""
    if not isinstance(reintentos, int) or isinstance(reintentos, bool):
        raise TypeError("reintentos debe ser un entero")
    if reintentos < 0:
        raise ValueError("reintentos no puede ser negativo")
    return 17 + 3 * min(reintentos, 6)


def espera_total_segundos(reintentos: int) -> int:
    """Suma las esperas de los primeros reintentos, hasta el máximo de seis.

    Con cero reintentos no hay esperas; los valores superiores a seis no
    añaden más esperas a la suma.
    """
    # Reutiliza la validación de la política antes de limitar el número.
    espera_segundos(reintentos)
    cantidad = min(reintentos, 6)
    return sum(espera_segundos(indice) for indice in range(1, cantidad + 1))


def cabe_en_presupuesto(reintentos: int, presupuesto_segundos: int) -> bool:
    """Indica si el tiempo acumulado cabe en el presupuesto inclusivo."""
    if not isinstance(presupuesto_segundos, int) or isinstance(presupuesto_segundos, bool):
        raise TypeError("presupuesto_segundos debe ser un entero")
    if presupuesto_segundos < 0:
        raise ValueError("presupuesto_segundos no puede ser negativo")
    return espera_total_segundos(reintentos) <= presupuesto_segundos


def planificar_reintentos(presupuesto_segundos: int, ejecutados: int) -> list[tuple[int, int]]:
    """Devuelve pares (número de reintento, espera) pendientes y asequibles.

    El presupuesto incluye las esperas ya ejecutadas y admite igualdad. Se
    presupone que los ejecutados forman un prefijo de la política; si ese
    prefijo ya excede el presupuesto, el resultado es vacío. Se limita el
    número ejecutado a seis, como en la política acumulada existente.
    Ambos argumentos deben ser enteros no negativos, excluidos los booleanos.
    """
    if not cabe_en_presupuesto(ejecutados, presupuesto_segundos):
        return []
    acumulado = espera_total_segundos(ejecutados)
    plan = []
    for numero in range(min(ejecutados, 6) + 1, 7):
        espera = espera_segundos(numero)
        if acumulado + espera > presupuesto_segundos:
            break
        plan.append((numero, espera))
        acumulado += espera
    return plan


def crear_checkpoint(presupuesto_segundos: int, ejecutados: int) -> str:
    """Serializa un prefijo realizable; el presupuesto se conserva al reanudar."""
    # El planificador histórico limita valores grandes; un checkpoint debe
    # representar exactamente un prefijo real, sin limitarlo silenciosamente.
    cabe = cabe_en_presupuesto(ejecutados, presupuesto_segundos)
    if ejecutados > 6 or not cabe:
        raise ValueError("prefijo incompatible con la política o el presupuesto")
    return json.dumps({"version": 1, "politica": "NACAR582",
                       "presupuesto_segundos": presupuesto_segundos,
                       "ejecutados": ejecutados,
                       "acumulado_segundos": espera_total_segundos(ejecutados)},
                      ensure_ascii=True, separators=(",", ":"))


def reanudar_checkpoint(datos: str) -> list[tuple[int, int]]:
    """Valida JSON no confiable y devuelve el plan pendiente del checkpoint v1.

    Se admiten exactamente los cinco campos del formato, sin claves repetidas.
    El acumulado debe coincidir con el prefijo y caber en el presupuesto.
    """
    if not isinstance(datos, str):
        raise TypeError("el checkpoint debe ser texto JSON")
    if len(datos) > 4096:
        raise ValueError("checkpoint demasiado grande")

    def objeto_sin_duplicados(pares):
        resultado = {}
        for clave, valor in pares:
            if clave in resultado:
                raise ValueError("clave duplicada en checkpoint")
            resultado[clave] = valor
        return resultado

    try:
        estado = json.loads(datos, object_pairs_hook=objeto_sin_duplicados)
    except (ValueError, RecursionError) as error:
        raise ValueError("checkpoint JSON inválido") from error
    campos = {"version", "politica", "presupuesto_segundos", "ejecutados", "acumulado_segundos"}
    if not isinstance(estado, dict) or estado.keys() != campos:
        raise ValueError("campos de checkpoint inválidos")
    if type(estado["version"]) is not int or estado["version"] != 1 or estado["politica"] != "NACAR582":
        raise ValueError("versión o política incompatible")
    presupuesto = estado["presupuesto_segundos"]
    ejecutados = estado["ejecutados"]
    acumulado = estado["acumulado_segundos"]
    for valor in (presupuesto, ejecutados, acumulado):
        if type(valor) is not int or valor < 0:
            raise ValueError("valores de checkpoint inválidos")
    if ejecutados > 6 or acumulado != espera_total_segundos(ejecutados) or acumulado > presupuesto:
        raise ValueError("estado de checkpoint incompatible")
    return planificar_reintentos(presupuesto, ejecutados)
