#!/usr/bin/env python3
"""Brings public-domain and CC0 business documents into tests/corpus/business/, cleaned.

    import-public-docs.py ORIGINALS_DIR [--fetch] [--check]

DOCUMENTS below lists each document: where it comes from, the day it was downloaded, the
SHA-256 of the original, and the licence statement of its source. ORIGINALS_DIR holds the
originals as <name>.docx; with --fetch, missing ones are downloaded from their URL (some
sites refuse scripts, so those originals are saved by hand). Every original must have the
listed SHA-256. The script then writes tests/corpus/business/<name>.docx with

  - the people taken out of the document properties (creator, last modified by, company,
    manager), and the custom properties, custom XML parts, comment authors (people.xml),
    sensitivity labels and thumbnail removed: each of these parts is found by the
    relationships to it and its content type, and by its usual name as part names compare;
  - every picture replaced by a grey placeholder of ours of the same pixel size, saved as
    PNG: agency seals, emblems and logos must not be reused, so no picture of the original
    is kept;
  - the attached template and the document variables removed from the settings;
  - the text replacements listed for the document, which put fictional names and addresses
    in place of a real person's. A replacement names the text by a pattern and the text's
    SHA-256, so the personal data it removes is never written into this repository;

and the document's row in the table of tests/corpus/SOURCES.md: URL, download date,
licence statement and the changes. It refuses documents with tracked changes, comments,
ActiveX controls, embedded fonts or objects, macros, mail merge sources, links to local
files and archives in which two entries name one part. With --check it writes nothing and
fails if a committed file differs from what it would write. Python standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import posixpath
import re
import string
import struct
import sys
import urllib.parse
import urllib.request
import zipfile
import zlib
from dataclasses import dataclass, field
from pathlib import Path
from xml.etree import ElementTree

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "tests" / "corpus"
OUT_DIR = CORPUS / "business"
SOURCES_MD = CORPUS / "SOURCES.md"

TABLE_HEADER = "| File | Origin | Licence | Changes |"
TABLE_TITLE = "## Public and openly licensed documents (`business/`)"

EMAIL = re.compile(rb"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
EMAIL_TEXT = re.compile(EMAIL.pattern.decode())
# A path into a user profile: C:\Users\<name>, /Users/<name>, file:///C:/Users/<name>/...
PROFILE_PATH = re.compile(rb"(?i)(\\|/)(users|documents and settings)(\\|/)")
# Addresses that name no person: fictional domains.
FICTIONAL = re.compile(rb"@example\.(com|gov|org)$")

RELS_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
# The same relationship types in ISO/IEC 29500 Strict (Part 1, §8.1).
STRICT_RELS_NS = "http://purl.oclc.org/ooxml/officeDocument/relationships"
# The namespaces of [Content_Types].xml and of .rels parts (ECMA-376 Part 2).
CONTENT_TYPES_NS = "{http://schemas.openxmlformats.org/package/2006/content-types}"
PACKAGE_RELS_NS = "{http://schemas.openxmlformats.org/package/2006/relationships}"


@dataclass(frozen=True)
class Source:
    """Where a group of documents comes from, and the licence statement that covers them."""

    licence: str
    statement_url: str
    quote: str


@dataclass(frozen=True)
class Replacement:
    """Text put in place of a real person's name or address. The pattern finds the text and
    the SHA-256 says which text it must be, so the personal data is not written here."""

    part: str
    pattern: str
    sha256: str
    new: str


@dataclass(frozen=True)
class PublicDoc:
    name: str
    source: str
    url: str
    downloaded: str
    # Of the document itself, also when it comes out of an archive.
    sha256: str
    # Generic mailboxes and template placeholders that may stay.
    emails: tuple[str, ...] = ()
    # Agency switchboards and the fictional numbers of sample forms that may stay.
    phones: tuple[str, ...] = ()
    replacements: tuple[Replacement, ...] = ()
    # What the row says about the replacements, without repeating the replaced text.
    replaced: str = ""
    # The document's path in the ZIP archive at url, when url names an archive.
    member: str = ""


PD_CC0 = "Public domain (work of the United States Government), and CC0 1.0 worldwide"

SOURCES: dict[str, Source] = {
    "GSA/idmanagement.gov": Source(
        PD_CC0,
        "https://github.com/GSA/idmanagement.gov/blob/e8a9a1a0c949673c4005b9b946394c616696772a/"
        "LICENSE.md",
        "As a work of the United States government, this project is in the public domain "
        "within the United States of America. Additionally, we waive copyright and related "
        "rights in the work worldwide through the CC0 1.0 Universal public domain dedication.",
    ),
    "18F/dol-whd-foh": Source(
        PD_CC0,
        "https://github.com/18F/dol-whd-foh/blob/b13697037c89d41342be4bce659fc740aca386c0/"
        "LICENSE.md",
        "As a work of the United States Government, this project is in the public domain "
        "within the United States. Additionally, we waive copyright and related rights in the "
        "work worldwide through the CC0 1.0 Universal public domain dedication.",
    ),
    "usds/techfar-hub-website-v3": Source(
        PD_CC0,
        "https://github.com/usds/techfar-hub-website-v3/blob/"
        "963cb7aa835bff3556c555f657e73877cd1116f4/LICENSE",
        "As a work of the United States Government, this project is in the public domain "
        "within the United States. Additionally, we waive copyright in the work worldwide "
        "through the CC0 1.0 Universal public domain dedication.",
    ),
    "18F/Head_Start_TTA_RFQ": Source(
        PD_CC0,
        "https://github.com/18F/Head_Start_TTA_RFQ/blob/95cd8592f7cf891422c92f480494777951885581/"
        "license.md",
        "As a work of the United States government, this project is in the public domain "
        "within the United States. Additionally, we waive copyright and related rights in the "
        "work worldwide through the CC0 1.0 Universal public domain dedication.",
    ),
    "cfpb/regulation-b-form-files": Source(
        PD_CC0,
        "https://github.com/cfpb/regulation-b-form-files/blob/"
        "45b0470a6420018a04892041c16592fca15a9489/TERMS.md",
        "As a work of the United States Government, this package (excluding any exceptions "
        "listed below) is in the public domain within the United States. Additionally, we "
        "waive copyright and related rights in the work worldwide through the CC0 1.0 "
        "Universal public domain dedication. (The only exception listed is Normalize.css.)",
    ),
    "cfpb/debt-collection-files": Source(
        PD_CC0,
        "https://github.com/cfpb/debt-collection-files/blob/"
        "4e421bdde2be1182e44f2dfe9683d120e2827a34/TERMS.md",
        "As a work of the United States Government, this package (excluding any exceptions "
        "listed below) is in the public domain within the United States. Additionally, we "
        "waive copyright and related rights in the work worldwide through the CC0 1.0 "
        "Universal public domain dedication.",
    ),
    "GSA/CFO.gov": Source(
        PD_CC0,
        "https://github.com/GSA/CFO.gov/blob/df9e99861dce40be9bb32dcbcbb106182100ca02/LICENSE.md",
        "As a work of the United States government, this project is in the public domain "
        "within the United States of America. Additionally, we waive copyright and related "
        "rights in the work worldwide through the CC0 1.0 Universal public domain dedication.",
    ),
    "GSA/digitalgov-usability": Source(
        "Public domain (work of the United States Government); branding removed as asked",
        "https://github.com/GSA/digitalgov-usability/blob/"
        "3380b79f6647868c808b453689c138a937636278/about-us/index.html",
        "Unless otherwise noted, text, documents, and images on the Usability.gov website are "
        "in the public domain, are not copyrighted, and therefore may be copied and "
        "distributed at no cost. We ask that you please credit Usability.gov as the source of "
        "the material. [...] If you adapt or modify the materials in anyway, all Usability.gov "
        "and federal branding and logos must be removed.",
    ),
    "nara": Source(
        "Public domain (work of the United States Government), and CC0 1.0 worldwide",
        "https://www.archives.gov/global-pages/privacy.html",
        "Works of the U.S. Government that have been produced by the National Archives and "
        "Records Administration are in the public domain and may be copied and distributed "
        "without permission. These works are available for worldwide use and reuse under the "
        "terms of the Creative Commons CC0 1.0 Universal license.",
    ),
    "eu-commission": Source(
        "CC BY 4.0, \u00a9 European Union (attribution in THIRD-PARTY-NOTICES.md)",
        "https://commission.europa.eu/legal-notice_en",
        "Unless otherwise indicated (e.g. in individual copyright notices), content owned by "
        "the EU on this website is licensed under the Creative Commons Attribution 4.0 "
        "International (CC BY 4.0) licence. This means that reuse is allowed, provided "
        "appropriate credit is given and changes are indicated.",
    ),
    "dol-ebsa": Source(
        "Public domain (work of the United States Government)",
        "https://www.dol.gov/general/aboutdol/copyright",
        "Materials created by the federal government are generally part of the public domain "
        "and may be used, reproduced and distributed without permission.",
    ),
}


# Archives of the European Commission that hold one document per language.
STATE_AID_FORMS = (
    "https://competition-policy.ec.europa.eu/document/download/66c3ba98-4ddf-48a7-a7c4-"
    "24234db8672b_en?filename=forms_notifications_and_reporting_part_I.zip"
)
DATA_PROCESSING_CLAUSES = (
    "https://commission.europa.eu/document/download/a53e88fe-3b08-4e83-8587-4e3869ebb1c3_en"
    "?filename=strandard_contractual_clauses_eu-eea.zip"
)

DOCUMENTS: list[PublicDoc] = [
    PublicDoc(
        "contract-hhs-terms-and-conditions",
        "18F/Head_Start_TTA_RFQ",
        "https://raw.githubusercontent.com/18F/Head_Start_TTA_RFQ/"
        "95cd8592f7cf891422c92f480494777951885581/appendices/Appendix-1-Terms-and-Conditions.docx",
        "2026-10-13",
        "d1a5a0ce9f514e7e780147e37b30c41c4e99f8fec46127e3814b63b8a8d759fa",
    ),
    PublicDoc(
        "contract-hhs-performance-work-statement",
        "18F/Head_Start_TTA_RFQ",
        "https://raw.githubusercontent.com/18F/Head_Start_TTA_RFQ/"
        "95cd8592f7cf891422c92f480494777951885581/appendices/Appendix-4-OHS-PWS-Template.docx",
        "2026-10-13",
        "e69482e3410da4221e4f770676c216a42d58b7237f248030d29b5885c4718afe",
    ),
    PublicDoc(
        "contract-hhs-teaming-agreement",
        "18F/Head_Start_TTA_RFQ",
        "https://raw.githubusercontent.com/18F/Head_Start_TTA_RFQ/"
        "95cd8592f7cf891422c92f480494777951885581/appendices/Appendix-7-CTA.docx",
        "2026-10-13",
        "be885603595cfbe1121b05584816e7c03bab359b2a036746ae9c38beb71a2a7c",
    ),
    PublicDoc(
        "contract-usds-agile-task-order",
        "usds/techfar-hub-website-v3",
        "https://raw.githubusercontent.com/usds/techfar-hub-website-v3/"
        "963cb7aa835bff3556c555f657e73877cd1116f4/tech-far-hub/static/assets/files/"
        "Agile%20Task%20Order%20Example.docx",
        "2026-10-13",
        "7ee66926457efcf4ca7960debe4765a747d204fe5bf9625c4c342938ec1fd979",
    ),
    PublicDoc(
        "contract-usds-statement-of-objectives",
        "usds/techfar-hub-website-v3",
        "https://raw.githubusercontent.com/usds/techfar-hub-website-v3/"
        "963cb7aa835bff3556c555f657e73877cd1116f4/tech-far-hub/static/assets/files/"
        "DigitalServiceSOO.docx",
        "2026-10-13",
        "57a9463aed08f8d3d4e0499b148b4792df5a845fe9e6e6bc1a41df5f88ca2951",
    ),
    PublicDoc(
        "contract-gsa-user-agreement",
        "GSA/idmanagement.gov",
        "https://raw.githubusercontent.com/GSA/idmanagement.gov/"
        "e8a9a1a0c949673c4005b9b946394c616696772a/docs/template-pua.docx",
        "2026-10-13",
        "056d8b2cf75945e16314275d7a4f286c04b01a7dde8a2fd8b792b7a7ab044d6d",
    ),
    PublicDoc(
        "contract-hhs-sow-usability-testing",
        "GSA/digitalgov-usability",
        "https://raw.githubusercontent.com/GSA/digitalgov-usability/"
        "3380b79f6647868c808b453689c138a937636278/sites/default/files/"
        "sow-template-usability-task-order-request_0.docx",
        "2026-10-13",
        "aea5665a56b8d9c71b73aff3b4004b2210fa5a7f5c152f49bac3503e71cb21e3",
    ),
    PublicDoc(
        "report-gsa-certificate-policy",
        "GSA/idmanagement.gov",
        "https://raw.githubusercontent.com/GSA/idmanagement.gov/"
        "e8a9a1a0c949673c4005b9b946394c616696772a/docs/fpki-x509-cert-policy-common.docx",
        "2026-10-13",
        "4faf9bf3b14fefcec2c1363ed13ccf7bcf50075d584a99c376326401ade45fc5",
        emails=('fpki@gsa.gov',),
    ),
    PublicDoc(
        "report-dol-field-handbook-15",
        "18F/dol-whd-foh",
        "https://raw.githubusercontent.com/18F/dol-whd-foh/"
        "b13697037c89d41342be4bce659fc740aca386c0/src/docx/15.docx",
        "2026-10-13",
        "9f91824659287d420688546f8196067eff6cd4dd1aa04c062da6e2be99780168",
    ),
    PublicDoc(
        "report-dol-field-handbook-11",
        "18F/dol-whd-foh",
        "https://raw.githubusercontent.com/18F/dol-whd-foh/"
        "b13697037c89d41342be4bce659fc740aca386c0/src/docx/11.docx",
        "2026-10-13",
        "312791c557fd099c8f968ab3fe3983df613c9626a96d0741210d909d9b640b16",
    ),
    PublicDoc(
        "report-gsa-csp-capabilities",
        "GSA/idmanagement.gov",
        "https://raw.githubusercontent.com/GSA/idmanagement.gov/"
        "e8a9a1a0c949673c4005b9b946394c616696772a/docs/"
        "credential-service-provider-capabilities-template-feb-2025.docx",
        "2026-10-13",
        "efafbc4e73c44992d14171c2124bbfc472fd9959c5fe4c6e37bd440d04e5d92a",
    ),
    PublicDoc(
        "report-hhs-usability-test-report",
        "GSA/digitalgov-usability",
        "https://raw.githubusercontent.com/GSA/digitalgov-usability/"
        "3380b79f6647868c808b453689c138a937636278/sites/default/files/"
        "report-template-usability-test-long.docx",
        "2026-10-13",
        "b0891db4156cc425f6a4010f45a2235bf3d10702befee12e3220957059a9147c",
        emails=('name@address.gov',),
    ),
    PublicDoc(
        "report-hhs-usability-test-plan",
        "GSA/digitalgov-usability",
        "https://raw.githubusercontent.com/GSA/digitalgov-usability/"
        "3380b79f6647868c808b453689c138a937636278/sites/default/files/usability-test-plan.docx",
        "2026-10-13",
        "28d8b58938bada405bd2c9bbad5bb44fba8f895f685fd1be0f5993981ebab9ab",
    ),
    PublicDoc(
        "report-usds-tech-challenge-playbook",
        "usds/techfar-hub-website-v3",
        "https://raw.githubusercontent.com/usds/techfar-hub-website-v3/"
        "963cb7aa835bff3556c555f657e73877cd1116f4/tech-far-hub/static/assets/files/"
        "USDS-Tech-Challenge-Playbook.docx",
        "2026-10-13",
        "41134888bdc37d1472e851eb7721d1490aad93ac107d959cbf7b8a307545e2d9",
    ),
    PublicDoc(
        "form-cfo-site-visit-checklists",
        "GSA/CFO.gov",
        "https://raw.githubusercontent.com/GSA/CFO.gov/df9e99861dce40be9bb32dcbcbb106182100ca02/"
        "assets/files/Panel1_DHS-Site-Visit-Checklists-and-Guidelines-6-27-2016.docx",
        "2026-10-13",
        "e407e0f31baebd21e7c808c631404403c7764134b978342e2f76bde64d6d0b06",
    ),
    PublicDoc(
        "form-gsa-dira-dias-template",
        "GSA/idmanagement.gov",
        "https://raw.githubusercontent.com/GSA/idmanagement.gov/"
        "e8a9a1a0c949673c4005b9b946394c616696772a/docs/playbook-dira-dias-template.docx",
        "2026-10-13",
        "d750a4f7f9acc1e929366c54bcad80bd45e51cfa96ee982e8412a794093529b3",
    ),
    PublicDoc(
        "form-gsa-pacs-series-form",
        "GSA/idmanagement.gov",
        "https://raw.githubusercontent.com/GSA/idmanagement.gov/"
        "e8a9a1a0c949673c4005b9b946394c616696772a/docs/archived/pacsapp-seriesform_20231211.docx",
        "2026-10-13",
        "78559552a97a9e28a6141da5774ba6e5523a411424a046c3708906969de36327",
    ),
    PublicDoc(
        "form-gsa-reseller-form",
        "GSA/idmanagement.gov",
        "https://raw.githubusercontent.com/GSA/idmanagement.gov/"
        "e8a9a1a0c949673c4005b9b946394c616696772a/docs/fips201ep-resellerform.docx",
        "2026-10-13",
        "1e875e99654deabb52be7da9061f7ae4d67f9064b9704b344c91570ac94c0263",
    ),
    PublicDoc(
        "form-cfpb-credit-form-c1",
        "cfpb/regulation-b-form-files",
        "https://raw.githubusercontent.com/cfpb/regulation-b-form-files/"
        "45b0470a6420018a04892041c16592fca15a9489/print/Form%20C-1.docx",
        "2026-10-13",
        "c87a3d025784008c22a89597d73a21a3a077a5718e3fbde698462bcfb7d966d6",
    ),
    PublicDoc(
        "form-cfpb-credit-form-c3-es",
        "cfpb/regulation-b-form-files",
        "https://raw.githubusercontent.com/cfpb/regulation-b-form-files/"
        "45b0470a6420018a04892041c16592fca15a9489/print_es/Form%20C-3_es.docx",
        "2026-10-13",
        "dd6b56bb83cf0044ba9aac850a13b64bedefc7a1fe58b469741d03b08dcc01d5",
    ),
    PublicDoc(
        "letter-cfpb-validation-notice",
        "cfpb/debt-collection-files",
        "https://raw.githubusercontent.com/cfpb/debt-collection-files/"
        "4e421bdde2be1182e44f2dfe9683d120e2827a34/print/"
        "cfpb_debt-collection_model-validation-notice_en.docx",
        "2026-10-13",
        "69130944438145d855bfb4b753eb435d0a6889fbaa2f883d719edf4392293fc7",
        phones=("(800) 123-4567",),
    ),
    PublicDoc(
        "letter-cfpb-validation-notice-es",
        "cfpb/debt-collection-files",
        "https://raw.githubusercontent.com/cfpb/debt-collection-files/"
        "4e421bdde2be1182e44f2dfe9683d120e2827a34/print/"
        "cfpb_debt-collection_model-validation-notice_es.docx",
        "2026-10-13",
        "f54fb8f6777d4c50fbfed8327b490b1dfcb53172f54c985a8e4f1b3e711c3ce2",
        phones=("(800) 123-4567",),
    ),
    PublicDoc(
        "letter-nara-ibr-request",
        "nara",
        "https://www.archives.gov/files/federal-register/write/handbook/ibr-request-letter.docx",
        "2026-10-13",
        "00592dda8af498619cd13b70e8017d9f398f5d01569a565df202d5fb046ac0e3",
    ),
    PublicDoc(
        "letter-nara-liaison",
        "nara",
        "https://www.archives.gov/files/federal-register/write/handbook/liaison-letter.docx",
        "2026-10-13",
        "8d55fbbd200d31fa29277ac790623a05ff1863fc35c9467569e3ab6cb6c1b5ec",
    ),
    PublicDoc(
        "letter-cfo-controller-alert",
        "GSA/CFO.gov",
        "https://raw.githubusercontent.com/GSA/CFO.gov/df9e99861dce40be9bb32dcbcbb106182100ca02/"
        "wp-content/uploads/2016/03/Controller-Alert-Grants-Closeout-7.24.12.docx",
        "2026-10-13",
        "cfc847644c61ed9f73a937323481cd7d641de8a3368f5d044b1372b2c6480b66",
        replacements=(
            Replacement(
                "word/document.xml",
                r"please contact [^<]{1,60} at ",
                "2cb004b81f921422827ae3bc8c5d998c2f3c603e22645d22aa64ad4c9cf923b9",
                "please contact the Office of Federal Financial Management at ",
            ),
            Replacement(
                "word/document.xml",
                r"<w:t>[^<@]{1,40}@omb\.eop\.gov</w:t>",
                "cbb1c8bdef7278e8bed369fc2ae4ee8094af64894273afb9e7e27140fe55ce97",
                "<w:t>controller@example.gov</w:t>",
            ),
            Replacement(
                "word/_rels/document.xml.rels",
                r'mailto:[^"]{1,40}@omb\.eop\.gov',
                "485bc028f27b988a8cd4b25d4572fccdce0a128819760b5cd9d7337be88a7a43",
                "mailto:controller@example.gov",
            ),
        ),
        replaced="text: a staff member's name and e-mail address replaced by a fictional office "
        "and address (controller@example.gov)",
    ),
    PublicDoc(
        "letter-dol-cobra-general-notice",
        "dol-ebsa",
        "https://www.dol.gov/sites/dolgov/files/ebsa/laws-and-regulations/laws/cobra/"
        "model-general-notice.docx",
        "2026-10-12",
        "6454fb928212681be6c95fa441d008b8dc87aa55c17779cbbd07f3d7eaf87086",
        emails=('ebsa.opr@dol.gov',),
        # EBSA's and HealthCare.gov's toll-free numbers.
        phones=("1-866-444-3272", "1-800-318-2596"),
    ),
    PublicDoc(
        "letter-dol-cobra-election-notice",
        "dol-ebsa",
        "https://www.dol.gov/sites/dolgov/files/ebsa/laws-and-regulations/laws/cobra/"
        "model-election-notice.docx",
        "2026-10-12",
        "f32605b26909a009244ba569282de6a8dfbe76641c5d81c5c48ee8a9b6e55ff0",
        emails=('ebsa.opr@dol.gov',),
        # EBSA's and HealthCare.gov's toll-free numbers.
        phones=("1-866-444-3272", "1-800-318-2596"),
    ),
    PublicDoc(
        "form-ec-state-aid-notification-de",
        "eu-commission",
        STATE_AID_FORMS,
        "2026-10-13",
        "fa0f4f244947c7cc38cd4d7dc88c487bf084f9ca9811558ac44decadeec8f3e0",
        member="Part_I_de.docx",
    ),
    PublicDoc(
        "form-ec-state-aid-notification-bg",
        "eu-commission",
        STATE_AID_FORMS,
        "2026-10-13",
        "face480403779ab778823eb227392d596fc09890cba2cbdc396e8c796e76694c",
        member="Part_I_bg.docx",
    ),
    PublicDoc(
        "form-ec-state-aid-notification-el",
        "eu-commission",
        STATE_AID_FORMS,
        "2026-10-13",
        "6345956446d8eb8c70f0656cb57d70ddec2a28cb26dde6c30264e0a3f31979fb",
        member="Part_I_el.docx",
    ),
    PublicDoc(
        "contract-ec-data-processing-clauses-de",
        "eu-commission",
        DATA_PROCESSING_CLAUSES,
        "2026-10-13",
        "147e427694a4c3aee34b8a7cf7f1dfb8df53555f84ee7112bead02d9580cd52e",
        member="DE Annex Standard Contractual Clauses EU-EEA.docx",
    ),
    PublicDoc(
        "contract-ec-data-processing-clauses-bg",
        "eu-commission",
        DATA_PROCESSING_CLAUSES,
        "2026-10-13",
        "1372a8203b2141f14744bced14aaf2f79a61f08053b57a77a3ee92edd10bdc8d",
        member="BG Annex Strandard Contractual Clauses EU-EEA.docx",
    ),
    PublicDoc(
        "contract-ec-data-processing-clauses-el",
        "eu-commission",
        DATA_PROCESSING_CLAUSES,
        "2026-10-13",
        "ce7a060799c50a7ad08f96ee1911fcde3e50f68b5a50458877c963132baf953e",
        member="EL Annex Standard Contractual Clauses EU-EEA.docx",
    ),
]


# --- pictures -------------------------------------------------------------------------


def image_size(data: bytes) -> tuple[int, int] | None:
    """Pixel size of a PNG, JPEG, GIF or BMP picture; None for other formats."""
    if data[:8] == b"\x89PNG\r\n\x1a\n" and len(data) >= 24:
        return struct.unpack(">II", data[16:24])
    if data[:6] in (b"GIF87a", b"GIF89a") and len(data) >= 10:
        return struct.unpack("<HH", data[6:10])
    if data[:2] == b"BM" and len(data) >= 26:
        width, height = struct.unpack("<ii", data[18:26])
        return abs(width), abs(height)
    if data[:2] == b"\xff\xd8":
        position = 2
        while position + 9 < len(data):
            if data[position] != 0xFF:
                return None
            marker = data[position + 1]
            (length,) = struct.unpack(">H", data[position + 2 : position + 4])
            # Start-of-frame markers, which carry the size: C0 to CF but for C4, C8, CC.
            if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
                height, width = struct.unpack(">HH", data[position + 5 : position + 9])
                return width, height
            position += 2 + length
    return None


# Placeholder size for vector pictures (EMF, WMF): their drawing size, not pixels, sets the
# layout, and the document gives that size anyway.
VECTOR_SIZE = (96, 96)
MAX_SIDE = 2400


def placeholder_rows(width: int, height: int) -> bytes:
    """The grey pixels of a placeholder, one filter byte (0) before each row: a light grey
    field, a darker frame and two diagonals."""
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        if y in (0, height - 1):
            rows += b"\x90" * width
            continue
        row = bytearray(b"\xe0" * width)
        for x in {y * width // height, (height - 1 - y) * width // height}:
            if 0 <= x < width:
                row[x] = 0xB0
        row[0] = row[-1] = 0x90
        rows += row
    return bytes(rows)


def clamp(width: int, height: int) -> tuple[int, int]:
    return max(1, min(width, MAX_SIDE)), max(1, min(height, MAX_SIDE))


def placeholder(width: int, height: int) -> bytes:
    """The PNG that stands in for every picture: placeholder_rows in 8-bit grey."""
    width, height = clamp(width, height)

    def chunk(kind: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + kind
            + body
            + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(placeholder_rows(width, height), 9))
        + chunk(b"IEND", b"")
    )


def is_placeholder(data: bytes) -> bool:
    """Whether a PNG holds exactly the placeholder's pixels, whatever zlib compressed them."""
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return False
    position, header, idat = 8, b"", b""
    while position + 8 <= len(data):
        (length,) = struct.unpack(">I", data[position : position + 4])
        kind, body = data[position + 4 : position + 8], data[position + 8 : position + 8 + length]
        if kind == b"IHDR":
            header = body
        elif kind == b"IDAT":
            idat += body
        position += 12 + length
    if len(header) != 13:
        return False
    width, height = struct.unpack(">II", header[:8])
    if header[8:] != bytes((8, 0, 0, 0, 0)) or (width, height) != clamp(width, height):
        return False
    try:
        return zlib.decompress(idat) == placeholder_rows(width, height)
    except zlib.error:
        return False


# --- cleaning --------------------------------------------------------------------------

MEDIA = re.compile(r"^word/(?:[^/]+/)*media/[^/]+$")
# Elements of tracked changes, and any author attribute.
REVISION = re.compile(
    rb"<w:(ins|del|moveFrom|moveTo|rPrChange|pPrChange|sectPrChange|tblPrChange|trPrChange|"
    rb"tcPrChange|numberingChange|cellIns|cellDel|cellMerge)\b|\bw:author="
)
# Phone numbers as North American forms write them.
PHONE = re.compile(r"\(\d{3}\)\s?\d{3}[-.\s]\d{4}|\b1?-?\d{3}[-.]\d{3}[-.]\d{4}\b")
# The parts that hold people's data or settings the cleaning edits, by role. A part has a
# role by the type of a relationship that names it or by its content type, whatever its
# name, and by its usual name as part names compare (part_key): an OPC reader finds it any
# of these ways (#375).
ROLE_TYPES = {
    "/metadata/core-properties": "core",
    "/extended-properties": "app",
    "/custom-properties": "custom",
    "/metadata/thumbnail": "thumbnail",
    "/customXml": "customxml",
    "/customXmlProps": "customxml",
    "/people": "people",
    # docMetadata/LabelInfo.xml holds sensitivity labels, as docProps/custom.xml may.
    "/classificationlabels": "label",
    "/comments": "comments",
    "/settings": "settings",
}
# The same by content type (media_type), from the Override elements of [Content_Types].xml.
ROLE_CONTENT_TYPES = {
    "application/vnd.openxmlformats-package.core-properties+xml": "core",
    "application/vnd.openxmlformats-officedocument.extended-properties+xml": "app",
    "application/vnd.openxmlformats-officedocument.custom-properties+xml": "custom",
    "application/vnd.openxmlformats-officedocument.customxmlproperties+xml": "customxml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.people+xml": "people",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml": "comments",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml": "settings",
    "application/vnd.ms-office.classificationlabels+xml": "label",
}
ROLE_NAMES = {
    "docprops/core.xml": "core",
    "docprops/app.xml": "app",
    "docprops/custom.xml": "custom",
    "word/people.xml": "people",
    "word/comments.xml": "comments",
    "word/settings.xml": "settings",
}
ROLE_FOLDERS = {"docprops/thumbnail.": "thumbnail", "customxml/": "customxml",
                "docmetadata/": "label"}
# The roles whose parts the cleaning drops, with what SOURCES.md calls them.
DROPPED_ROLES = {
    "custom": "custom properties",
    "thumbnail": "thumbnail",
    "customxml": "custom XML parts",
    "people": "comment authors",
    "label": "sensitivity label",
}


class Refused(Exception):
    """A document that must not enter the corpus as it is; the message says why."""


# What the corpus never takes, found by what a part is rather than by its usual name: OPC
# compares part names without regard to case (ECMA-376 Part 2, §9.1.1.1), and a package may
# put a part anywhere and say what it is by its content type and by the relationships that
# point to it.
EMBEDDED_FOLDERS = ("word/fonts/", "word/embeddings/", "word/activex/")
ACTIVEX_TYPES = ("application/vnd.ms-office.activex+xml", "application/vnd.ms-office.activex")
EMBEDDED_RELATIONSHIPS = tuple(
    f"{namespace}/{kind}"
    for namespace in (RELS_NS, STRICT_RELS_NS)
    for kind in ("control", "font", "oleObject", "package")
) + ("http://schemas.microsoft.com/office/2006/relationships/activeXControlBinary",)


class Unreadable(Exception):
    """A package part that embedded_parts must read cannot be read as OPC allows it."""


class _Builder(ElementTree.TreeBuilder):
    """A tree builder that notes a document type declaration, which OPC forbids in package
    parts (ECMA-376 Part 2, §8.1.4) and which could give attributes defaults, such as a
    TargetMode of External, that the markup does not show."""

    doctype_seen = False

    def doctype(self, name, pubid, system):
        self.doctype_seen = True


def package_xml(name: str, data: bytes, root: str) -> ElementTree.Element:
    """[Content_Types].xml or a .rels part, read as XML: the parser resolves character
    references and namespace prefixes, and reads UTF-16 (ECMA-376 Part 2 allows it), so
    that no spelling of the markup hides a part. The part is refused when it is not
    well-formed, declares a document type, or its root is not the element root names (with
    its namespace), as officewright-opc refuses it."""
    builder = _Builder()
    parser = ElementTree.XMLParser(target=builder)
    try:
        parser.feed(data)
        element = parser.close()
    except (ElementTree.ParseError, ValueError, LookupError) as error:
        raise Unreadable(f"{name} is not well-formed XML ({error})") from error
    if builder.doctype_seen:
        raise Unreadable(f"{name} declares a document type, which OPC forbids")
    if element.tag != root:
        raise Unreadable(f"{name} has the root element {element.tag}, not {root}")
    return element


def media_type(value: str) -> str:
    """A content type without its parameters, in lower case: types, subtypes and parameter
    names compare without regard to case (RFC 2045, §5.1)."""
    return value.split(";", 1)[0].strip().lower()


def embedded_parts(parts: dict[str, bytes]) -> list[str]:
    """The names of the parts that are embedded fonts or objects or ActiveX controls, as the
    package spells them: by folder, by content type and by relationship type. Raises
    Unreadable when [Content_Types].xml or a .rels part cannot be read (package_xml), or the
    package has two content types streams whose names differ only in case."""
    by_lower = {name.lower(): name for name in parts}
    found = {name for name in parts if name.lower().startswith(EMBEDDED_FOLDERS)}

    def add(name: str) -> None:
        found.add(by_lower.get(name.lower(), name))

    # Part names compare without regard to case, that of the content types stream too, so
    # two streams whose names differ only in case are one name twice.
    streams = [name for name in parts if name.lower() == "[content_types].xml"]
    if len(streams) > 1:
        raise Unreadable(f"the package has {len(streams)} content types streams: "
                         + ", ".join(sorted(streams)))
    for types_name in streams:
        types = package_xml(types_name, parts[types_name], f"{CONTENT_TYPES_NS}Types")
        for element in types.iter(f"{CONTENT_TYPES_NS}Override"):
            if media_type(element.get("ContentType", "")) in ACTIVEX_TYPES:
                add(element.get("PartName", "").lstrip("/"))
        for element in types.iter(f"{CONTENT_TYPES_NS}Default"):
            # Extensions compare as part names do: percent-decoded, without regard to case.
            extension = urllib.parse.unquote(element.get("Extension", "")).lower()
            if extension and media_type(element.get("ContentType", "")) in ACTIVEX_TYPES:
                found.update(
                    name for name in parts
                    if urllib.parse.unquote(name).lower().endswith("." + extension)
                )
    index = part_index(parts)
    for rels in (name for name in parts if name.lower().endswith(".rels")):
        relationships = package_xml(rels, parts[rels], f"{PACKAGE_RELS_NS}Relationships")
        for element in relationships.iter(f"{PACKAGE_RELS_NS}Relationship"):
            if element.get("TargetMode") == "External":
                continue
            if element.get("Type", "").strip(URI_SPACE) not in EMBEDDED_RELATIONSHIPS:
                continue
            # A part the package lacks is named only when its name is plainly a part name; any
            # other target is refused by external_link_problems, which does not name it.
            target = element.get("Target", "").strip(URI_SPACE)
            held = internal_part(index, rels, target)
            name = held or part_target(rels, target)
            if held or (part_reference(urllib.parse.unquote(target))
                        and PLAIN_PART_NAME.match(name)):
                add(name)
    return sorted(found)


def part_target(rels_part: str, target: str) -> str:
    """The part a relationship's target names, from the .rels part that holds it."""
    if target.startswith("/"):
        return target.lstrip("/")
    source_dir = posixpath.dirname(posixpath.dirname(rels_part))  # word/_rels/x.rels -> word
    return posixpath.normpath(posixpath.join(source_dir, target))


# The schemes an external relationship may link to: a link anywhere else (file:, a UNC or
# drive path, a relative path) names a place on someone's machine or network.
LINK_SCHEMES = re.compile(r"(?i)(https?|mailto):")


# A part name made only of letters, digits, ".", "_" and "-" in segments, which names no
# place outside the package and is safe to name in a message.
PLAIN_PART_NAME = re.compile(r"[A-Za-z0-9_-][A-Za-z0-9._-]*(?:/[A-Za-z0-9_-][A-Za-z0-9._-]*)*$")
# xsd:anyURI, the type of Target and Type, collapses whitespace: they are read without it
# at their ends.
URI_SPACE = " \t\r\n"


def part_index(parts: dict[str, bytes]) -> dict[str, str]:
    """The package's part names as part names compare (percent-decoded, without regard to
    case), to their names as the package spells them."""
    return {urllib.parse.unquote(part).lower(): part for part in parts}


def part_reference(target: str) -> bool:
    """Whether a target, percent-decoded, is written as a part name: named segments, "." and
    ".." only before the first of them, and no backslash, colon, query or fragment. Anything
    else could climb back from a local path to a part ("D:/x/../../a.xml") or name no part."""
    if not target or target.startswith("//") or any(c in target for c in "\\:?#"):
        return False
    named = False
    for segment in target.lstrip("/").split("/"):
        if segment in (".", ".."):
            if named:
                return False
        elif not segment:
            return False
        else:
            named = True
    return named


def internal_part(index: dict[str, str], rels: str, target: str) -> str | None:
    """The part of the package (index: part_index) that an internal relationship's target
    names, or None: the target, without the whitespace xsd:anyURI collapses and
    percent-decoded, must be written as a part name (part_reference) and name a part the
    package holds. A scheme, a drive, a server, a backslash, a query, a way out of the package
    or a missing part all name none, however they are spelled."""
    target = target.strip(URI_SPACE)
    if not part_reference(urllib.parse.unquote(target)):
        return None
    return index.get(urllib.parse.unquote(part_target(rels, target)).lower())


def part_roles(parts: dict[str, bytes]) -> dict[str, str]:
    """The role (ROLE_TYPES) of each part that has one, by the relationships that name it or
    its content type, whatever its name, else by its usual name or folder as part names
    compare. A part that one of these makes comments or a part to drop keeps that role; the
    .rels part of a part to drop is dropped with it. Raises Unreadable when
    [Content_Types].xml or a .rels part cannot be read."""
    roles: dict[str, str] = {}
    index = part_index(parts)
    by_key = {part_key(name): name for name in parts}

    def add(name: str, role: str) -> None:
        if roles.get(name) not in (*DROPPED_ROLES, "comments"):
            roles[name] = role

    for name in parts:
        key = part_key(name)
        role = ROLE_NAMES.get(key) or next(
            (role for folder, role in ROLE_FOLDERS.items() if key.startswith(folder)), None
        )
        if role:
            add(name, role)
    for types_name in (name for name in parts if name.lower() == "[content_types].xml"):
        types = package_xml(types_name, parts[types_name], f"{CONTENT_TYPES_NS}Types")
        for element in types.iter(f"{CONTENT_TYPES_NS}Override"):
            role = ROLE_CONTENT_TYPES.get(media_type(element.get("ContentType", "")))
            held = by_key.get(part_key(element.get("PartName", "").lstrip("/")))
            if role and held:
                add(held, role)
    for rels in (name for name in parts if name.lower().endswith(".rels")):
        root = package_xml(rels, parts[rels], f"{PACKAGE_RELS_NS}Relationships")
        for element in root.iter(f"{PACKAGE_RELS_NS}Relationship"):
            if element.get("TargetMode") == "External":
                continue
            kind = element.get("Type", "").strip(URI_SPACE)
            role = next((r for suffix, r in ROLE_TYPES.items() if kind.endswith(suffix)), None)
            held = internal_part(index, rels, element.get("Target", "")) if role else None
            if held:
                add(held, role)
    for name, role in list(roles.items()):
        if role in DROPPED_ROLES:
            folder, base = posixpath.split(name)
            rels = by_key.get(part_key(posixpath.join(folder, "_rels", base + ".rels")))
            if rels:
                roles[rels] = role
    return roles


def external_link_problems(parts: dict[str, bytes]) -> list[str]:
    """The .rels parts with a relationship to anything but a web or mail address or a part of
    the package, read as XML whatever the quoting, prefixes or character references (#331):
    an external one to another scheme, an internal one (no TargetMode, or Internal) whose
    target names no part, or one whose TargetMode is neither, or an attribute in a namespace,
    which OPC does not allow. Also an attached template the cleaning left, since its address
    can name a person (a personal site). The target itself is not named, since it may name a
    person's folder."""
    found = []
    index = part_index(parts)
    for rels in sorted(name for name in parts if name.lower().endswith(".rels")):
        root = package_xml(rels, parts[rels], f"{PACKAGE_RELS_NS}Relationships")
        for element in root.iter(f"{PACKAGE_RELS_NS}Relationship"):
            mode, target = element.get("TargetMode"), element.get("Target", "")
            if any(name.startswith("{") for name in element.attrib):
                found.append(f"{rels} has a relationship attribute in a namespace")
                break
            if mode not in (None, "Internal", "External"):
                found.append(f"{rels} has a target mode other than Internal or External")
                break
            if element.get("Type", "").strip(URI_SPACE).endswith("/attachedTemplate"):
                found.append(f"{rels} keeps an attached template")
                break
            if (mode == "External" and not LINK_SCHEMES.match(target.strip(URI_SPACE))) or (
                mode != "External" and internal_part(index, rels, target) is None
            ):
                found.append(f"{rels} links to a local file, another scheme or a missing part")
                break
    return found


def attribute(element: str, name: str) -> re.Match | None:
    """An attribute of an XML start tag, in double or single quotes: group 2 is its value."""
    return re.search(rf"""\b{name}\s*=\s*(["'])(.*?)\1""", element)


def edit_relationships(rels_part: str, xml: str, dropped: set[str], renamed: dict[str, str]) -> str:
    """Removes relationships to dropped parts and to attached templates; renames picture
    targets. External links are kept as they are: clean() refuses those to anything but web
    and mail addresses afterwards (external_link_problems), whatever their spelling. Targets
    are compared with the parts as part names compare (part_key)."""
    dropped_keys = {part_key(part) for part in dropped}
    renamed_keys = {part_key(old): new for old, new in renamed.items()}

    def edit(match: re.Match) -> str:
        element = match.group(0)
        kind = attribute(element, "Type")
        target = attribute(element, "Target")
        if not kind or not target:
            return element
        if kind.group(2).strip(URI_SPACE).endswith("/attachedTemplate"):
            return ""
        mode = attribute(element, "TargetMode")
        if mode and mode.group(2) == "External":
            return element
        part = part_key(part_target(rels_part, target.group(2).strip(URI_SPACE)))
        if part in dropped_keys:
            return ""
        if part in renamed_keys:
            old = target.group(2)
            new = (old[: len(old) - len(posixpath.basename(old))]
                   + posixpath.basename(renamed_keys[part]))
            return element.replace(target.group(0), f"Target={target.group(1)}{new}{target.group(1)}")
        return element

    return re.sub(r"<Relationship\b[^>]*/>", edit, xml)


def edit_content_types(xml: str, dropped: set[str], renamed: dict[str, str]) -> str:
    gone = {part_key(part) for part in dropped | set(renamed)}
    xml = re.sub(
        r'<Override\b[^>]*PartName="([^"]*)"[^>]*/>',
        lambda m: "" if part_key(m.group(1).lstrip("/")) in gone else m.group(0),
        xml,
    )
    if not re.search(r'<Default\b[^>]*Extension="png"', xml, re.I):
        xml = xml.replace(
            "</Types>", '<Default Extension="png" ContentType="image/png"/></Types>'
        )
    return xml


def empty_elements(xml: str, names: dict[str, str], emptied: list[str]) -> str:
    """The properties with these elements emptied; the labels of the ones that had a value
    go into emptied."""
    for name, label in names.items():
        if re.search(rf"<{name}>[^<]+</{name}>", xml):
            emptied.append(label)
        xml = re.sub(rf"<{name}>[^<]*</{name}>", f"<{name}></{name}>", xml)
    return xml


def text_of(name: str, data: bytes) -> str:
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise Refused(f"{name} is not UTF-8") from error


@dataclass
class Cleaned:
    data: bytes
    changes: list[str] = field(default_factory=list)


ASCII_LOWER = str.maketrans(string.ascii_uppercase, string.ascii_lowercase)


def part_key(name: str) -> str:
    """An entry name as OPC compares part names: a backslash read as a slash (as ZIP tools
    on Windows read it), percent-decoded, and without regard to ASCII case."""
    return urllib.parse.unquote(name.replace("\\", "/")).translate(ASCII_LOWER)


def repeated_parts(names: list[str]) -> list[str]:
    """The entry names that name one part, joined by "and" per part: the same name twice, or
    names that differ only as part names compare (part_key: case, percent-encoding, a
    backslash for a slash). The checks read the parts by name, so they would see only one
    entry of each."""
    by_part: dict[str, list[str]] = {}
    for name in names:
        by_part.setdefault(part_key(name), []).append(name)
    return [" and ".join(group) for group in by_part.values() if len(group) > 1]


def clean(original: bytes, doc: PublicDoc) -> Cleaned:
    """The document with people, pictures and private parts taken out (see the module
    docstring); refused when it holds something the corpus must not take."""
    with zipfile.ZipFile(io.BytesIO(original)) as archive:
        entries = [(info, archive.read(info)) for info in archive.infolist()]
    names = [info.filename for info, _ in entries]
    repeated = repeated_parts(names)
    if repeated:
        raise Refused(f"{doc.name}: " + "; ".join(f"entries {r} name one part" for r in repeated))
    try:
        embedded = embedded_parts({info.filename: data for info, data in entries})
    except Unreadable as error:
        raise Refused(f"{doc.name}: {error}") from error
    if embedded:
        raise Refused(f"{doc.name}: embeds {', '.join(embedded)}")
    try:
        roles = part_roles({info.filename: data for info, data in entries})
    except Unreadable as error:
        raise Refused(f"{doc.name}: {error}") from error
    if "comments" in roles.values():
        raise Refused(f"{doc.name}: has comments")
    for name in names:
        if "vbaproject" in name.lower():
            raise Refused(f"{doc.name}: has macros")
    for info, data in entries:
        # Tracked changes carry their authors, and Word could draw them as markup.
        if info.filename.endswith(".xml") and REVISION.search(data):
            raise Refused(f"{doc.name}: {info.filename} has tracked changes")
    dropped = {name for name, role in roles.items() if role in DROPPED_ROLES}
    renamed = {}
    for name in names:
        if MEDIA.match(name):
            stem = name.rsplit(".", 1)[0] if "." in posixpath.basename(name) else name
            renamed[name] = f"{stem}.png"
    # The entries the cleaning writes must not name one part either: a picture renamed to
    # .png next to another picture or entry of that name, in any case (a.jpeg and A.png).
    kept = [renamed.get(name, name) for name in names if name not in dropped]
    if len({part_key(name) for name in kept}) != len(kept):
        raise Refused(f"{doc.name}: a picture's new name would name one part with another entry")
    changes = []
    people = []
    if dropped:
        kinds = sorted({DROPPED_ROLES[roles[n]] for n in dropped})
        changes.append("removed: " + ", ".join(kinds))
    if renamed:
        changes.append(
            f"{len(renamed)} picture{'s' if len(renamed) != 1 else ''} replaced by a grey "
            "placeholder of the same pixel size (no agency seal, emblem or logo is kept)"
        )
    out = []
    pending = list(doc.replacements)
    settings_removed: list[str] = []
    for info, data in entries:
        name = info.filename
        if name in dropped:
            continue
        if name in renamed:
            size = image_size(data) or VECTOR_SIZE
            data = placeholder(*size)
            info = clone(info, renamed[name])
        elif name.endswith(".rels"):
            data = edit_relationships(name, text_of(name, data), dropped, renamed).encode("utf-8")
        elif name == "[Content_Types].xml":
            data = edit_content_types(text_of(name, data), dropped, renamed).encode("utf-8")
        elif roles.get(name) == "core":
            names_ = {"dc:creator": "creator", "cp:lastModifiedBy": "last modified by"}
            data = empty_elements(text_of(name, data), names_, people).encode("utf-8")
        elif roles.get(name) == "app":
            names_ = {"Company": "company", "Manager": "manager", "HyperlinkBase": "hyperlink base"}
            data = empty_elements(text_of(name, data), names_, people).encode("utf-8")
        elif roles.get(name) == "settings":
            text = text_of(name, data)
            if "<w:mailMerge" in text:
                raise Refused(f"{doc.name}: has a mail merge data source")
            edited = text
            for pattern, what in (
                (r"<w:attachedTemplate\b[^>]*/>", "attached template"),
                (r"<w:docVars>.*?</w:docVars>", "document variables"),
            ):
                edited, count = re.subn(pattern, "", edited, flags=re.S)
                if count:
                    settings_removed.append(what)
            data = edited.encode("utf-8")
        for replacement in [r for r in pending if r.part == name]:
            data = replace(doc.name, name, data, replacement)
            pending.remove(replacement)
        out.append((info, data))
    if pending:
        raise Refused(f"{doc.name}: no part {pending[0].part} for a replacement")
    # What the cleaning keeps, read as XML: an external link it did not see in some spelling
    # of the markup is refused here (#331). An attached template is removed above, so the
    # check comes after the edits.
    try:
        links = external_link_problems({info.filename: data for info, data in out})
    except Unreadable as error:
        raise Refused(f"{doc.name}: {error}") from error
    if links:
        raise Refused(f"{doc.name}: {'; '.join(links)}")
    if people:
        changes.insert(0, "document properties emptied: " + ", ".join(people))
    if settings_removed:
        changes.append(f"settings: {' and '.join(settings_removed)} removed")
    if doc.replaced:
        changes.append(doc.replaced)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for info, data in out:
            archive.writestr(info, data)
    return Cleaned(buffer.getvalue(), changes)


def replace(document: str, part: str, data: bytes, replacement: Replacement) -> bytes:
    """data with each match of the replacement's pattern whose SHA-256 is the listed one
    replaced; refused when there is none."""
    found = [
        m
        for m in re.finditer(replacement.pattern.encode("utf-8"), data)
        if hashlib.sha256(m.group(0)).hexdigest() == replacement.sha256
    ]
    if not found:
        raise Refused(f"{document}: {part} does not contain the text to replace")
    new = replacement.new.encode("utf-8")
    for match in reversed(found):
        data = data[: match.start()] + new + data[match.end() :]
    return data


def clone(info: zipfile.ZipInfo, name: str) -> zipfile.ZipInfo:
    copy = zipfile.ZipInfo(name, date_time=info.date_time)
    copy.compress_type = info.compress_type
    copy.external_attr = info.external_attr
    copy.create_system = info.create_system
    return copy


def problems(
    data: bytes, allowed_emails: tuple[str, ...] = (), allowed_phones: tuple[str, ...] = ()
) -> list[str]:
    """What a committed document must not hold; empty for a clean one."""
    found = []
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        found += [f"entries {repeated} name one part" for repeated in
                  repeated_parts(archive.namelist())]
        parts = {name: archive.read(name) for name in archive.namelist()}
        try:
            embedded = embedded_parts(parts)
            found += external_link_problems(parts)
            roles = part_roles(parts)
        except Unreadable as error:
            found.append(str(error))
            embedded, roles = [], {}
        found += [f"{name} is embedded" for name in embedded if name not in parts]
        for name, body in parts.items():
            if roles.get(name) in DROPPED_ROLES:
                found.append(f"{name} is still there")
            if roles.get(name) == "comments":
                found.append(f"{name} holds comments")
            if name in embedded or "vbaproject" in name.lower():
                found.append(f"{name} is embedded")
            if MEDIA.match(name) and not is_placeholder(body):
                found.append(f"{name} is not our placeholder")
            if not name.endswith((".xml", ".rels")):
                if not MEDIA.match(name):
                    found.append(f"{name} is a binary part")
                continue
            if REVISION.search(body):
                found.append(f"{name} has tracked changes or an author")
            # Text as it reads: the runs between two paragraph ends joined, so that a number
            # or address split over runs is seen, and text after a text box nested in a
            # paragraph is too (a nested paragraph ends inside the outer one).
            segments = [
                b"".join(re.findall(rb"<w:t(?: [^>]*)?>([^<]*)</w:t>", segment)).decode(
                    "utf-8", "replace"
                )
                for segment in body.split(b"</w:p>")
            ]
            for phone in sorted({p for text in segments for p in PHONE.findall(text)}):
                if phone not in allowed_phones:
                    found.append(f"{name} holds the phone number {phone}")
            if PROFILE_PATH.search(body):
                found.append(f"{name} holds a user profile path")
            addresses = set(EMAIL.findall(body))
            addresses |= {a.encode("utf-8") for text in segments for a in EMAIL_TEXT.findall(text)}
            for address in sorted(addresses):
                text = address.decode("utf-8", "replace")
                if text not in allowed_emails and not FICTIONAL.search(address):
                    found.append(f"{name} holds the address {text}")
            if roles.get(name) == "core":
                for element in ("dc:creator", "cp:lastModifiedBy"):
                    if re.search(rf"<{element}>[^<]+</{element}>".encode(), body):
                        found.append(f"{name}: {element} is not empty")
            if roles.get(name) == "app":
                for element in ("Company", "Manager"):
                    if re.search(rf"<{element}>[^<]+</{element}>".encode(), body):
                        found.append(f"{name}: {element} is not empty")
    return found


# --- SOURCES.md -------------------------------------------------------------------------


def sources_row(doc: PublicDoc, changes: list[str]) -> str:
    source = SOURCES[doc.source]
    cell = lambda text: text.replace("|", "\\|")
    return (
        f"| business/{doc.name}.docx "
        f"| {cell(doc.url)}{f', file {cell(doc.member)} in the archive' if doc.member else ''}, "
        f"downloaded {doc.downloaded}, original SHA-256 {doc.sha256} "
        f"| {cell(source.licence)}: \"{cell(source.quote)}\" ({source.statement_url}) "
        f"| {cell('; '.join(changes))} |"
    )


def with_rows(sources: str, rows: list[str]) -> str:
    """SOURCES.md with the table of these documents replaced by the rows."""
    table = "\n".join(
        [
            TABLE_TITLE,
            "",
            "Business documents of United States federal agencies (public domain, most also"
            " CC0) and of the European Commission (CC BY 4.0, credited in"
            " THIRD-PARTY-NOTICES.md), committed as `scripts/import-public-docs.py` cleans"
            " them: every row gives the URL, the download date, the original's SHA-256, the"
            " licence statement and what was changed.",
            "",
            TABLE_HEADER,
            "|------|--------|---------|---------|",
            *sorted(rows),
        ]
    )
    if TABLE_TITLE in sources:
        start = sources.index(TABLE_TITLE)
        end = sources.find("\n## ", start + 1)
        end = len(sources) if end < 0 else end + 1
        return sources[:start] + table + "\n\n" + sources[end:]
    anchor = sources.index("\n## Reference renders")
    return sources[: anchor + 1] + table + "\n" + sources[anchor:]


def same_package(committed: bytes, cleaned: bytes) -> bool:
    """Whether two packages hold the same parts: pictures compared by their pixels, since
    another zlib build compresses the same placeholder into other bytes."""
    if committed == cleaned:
        return True
    with zipfile.ZipFile(io.BytesIO(committed)) as a, zipfile.ZipFile(io.BytesIO(cleaned)) as b:
        if a.namelist() != b.namelist():
            return False
        for name in a.namelist():
            left, right = a.read(name), b.read(name)
            if MEDIA.match(name):
                if not (is_placeholder(left) and image_size(left) == image_size(right)):
                    return False
            elif left != right:
                return False
    return True


# --- main -------------------------------------------------------------------------------


def original(doc: PublicDoc, originals: Path, fetch: bool) -> bytes:
    path = originals / f"{doc.name}.docx"
    if not path.is_file():
        if not fetch:
            raise Refused(f"{doc.name}: {path} is missing (use --fetch, or save it from {doc.url})")
        request = urllib.request.Request(doc.url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(request, timeout=120) as response:
            data = response.read()
        if doc.member:
            try:
                with zipfile.ZipFile(io.BytesIO(data)) as archive:
                    data = archive.read(doc.member)
            except (zipfile.BadZipFile, KeyError) as error:
                raise Refused(f"{doc.name}: {doc.member} is not in the archive") from error
        path.write_bytes(data)
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != doc.sha256:
        raise Refused(f"{doc.name}: the original is not the listed file (SHA-256 differs)")
    return data


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("originals", type=Path, help="folder with the originals as <name>.docx")
    parser.add_argument("--fetch", action="store_true", help="download missing originals")
    parser.add_argument("--check", action="store_true", help="fail if a committed file differs")
    args = parser.parse_args()
    try:
        args.originals.mkdir(parents=True, exist_ok=True)
        results = []
        for doc in DOCUMENTS:
            cleaned = clean(original(doc, args.originals, args.fetch), doc)
            left = problems(cleaned.data, doc.emails, doc.phones)
            if left:
                raise Refused(f"{doc.name}: {'; '.join(left)}")
            results.append((doc, cleaned))
    except Refused as error:
        print(f"import-public-docs.py: {error}", file=sys.stderr)
        return 1
    rows = [sources_row(doc, cleaned.changes) for doc, cleaned in results]
    sources = with_rows(SOURCES_MD.read_text(encoding="utf-8"), rows)
    if args.check:
        stale = [
            doc.name
            for doc, cleaned in results
            if not (OUT_DIR / f"{doc.name}.docx").is_file()
            or not same_package((OUT_DIR / f"{doc.name}.docx").read_bytes(), cleaned.data)
        ]
        if sources != SOURCES_MD.read_text(encoding="utf-8"):
            stale.append("SOURCES.md")
        if stale:
            print(f"import-public-docs.py: differs: {', '.join(stale)}", file=sys.stderr)
            return 1
        print(f"import-public-docs.py: {len(results)} documents as committed")
        return 0
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for doc, cleaned in results:
        (OUT_DIR / f"{doc.name}.docx").write_bytes(cleaned.data)
    SOURCES_MD.write_text(sources, encoding="utf-8")
    print(f"import-public-docs.py: wrote {len(results)} documents")
    return 0


if __name__ == "__main__":
    sys.exit(main())
